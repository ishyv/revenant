//! Scoped previews retain captured core leases, never arbitrary native paths.
//! A random UUID token is the protocol read capability; only the owning UI scope
//! may dispose it. Creation requires an exact lease/UI scope match. Disposing a
//! scope blocks pending or future creations in that scope; scope IDs must not be
//! reused. Registry owner release does not invalidate an admitted preview lease.
//!
//! Text decoding reads at most 64 KiB. Protocol responses allocate at most 4 MiB,
//! support one byte range, and report the actual bounded slice honestly as 206.
//! No-range requests for larger resources also return a bounded 206 response.
//! The host formats token URLs for Tauri and authenticates the protocol origin;
//! it must poll these futures on its bounded native worker when the port blocks.
//! Locks are never held across awaits. Disposal prevents new reads; a read that
//! already captured a lease may finish, then releases its temporary reference.

use revenant_core::serde::{Deserialize, Serialize};
use revenant_core::serde_json::json;
use revenant_core::{Error, ResourceLease, Result};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

const TEXT_LIMIT: u64 = 64 * 1024;
const RESPONSE_LIMIT: u64 = 4 * 1024 * 1024;

/// Browser presentation category inferred from the captured media type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "revenant_core::serde", rename_all = "camelCase")]
pub enum PreviewKind {
    /// Bounded UTF-8 text, supplied inline in the descriptor.
    Text,
    /// Image resource served by the native protocol.
    Image,
    /// Audio resource served by the native protocol.
    Audio,
    /// Video resource served by the native protocol.
    Video,
    /// No supported browser presentation is known.
    Unsupported,
}

impl PreviewKind {
    fn token_prefix(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Client descriptor. For media, `url` initially contains the opaque token;
/// the Tauri host replaces it with its platform-specific protocol URL.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    crate = "revenant_core::serde",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct PreviewDescriptor {
    /// Opaque token used for reads and scope-authenticated disposal.
    pub id: String,
    /// Supported browser presentation category.
    pub kind: PreviewKind,
    /// Captured display name, never a native access path.
    pub name: String,
    /// Captured MIME type, or name-based fallback when absent.
    pub mime: String,
    /// UTF-8 text prefix for text previews only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Whether bytes remain beyond the displayed text prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Opaque media token before host formatting; native paths are never returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// One bounded native protocol response; the host adds HTTP headers and body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    crate = "revenant_core::serde",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct PreviewResponse {
    /// At most 4 MiB of raw bytes; pass directly to the protocol response body.
    pub body: Vec<u8>,
    /// Media type to use as Content-Type.
    pub mime: String,
    /// 200 for a whole no-range resource, otherwise 206 for a bounded slice.
    pub status: u16,
    /// Actual inclusive byte interval, present for partial responses.
    pub content_range: Option<String>,
    /// Captured total resource size in bytes.
    pub total: u64,
}

#[derive(Clone)]
struct Preview {
    scope: String,
    lease: ResourceLease,
    mime: String,
    kind: PreviewKind,
}

#[derive(Default)]
struct State {
    previews: HashMap<String, Preview>,
    disposed_scopes: HashSet<String>,
}

/// A thread-safe token registry retaining leases until explicit UI disposal.
/// Hosts must dispose previews/scopes with their UI ownership lifecycle.
#[derive(Default)]
pub struct PreviewStore {
    state: Mutex<State>,
}

impl PreviewStore {
    /// Creates an empty preview registry without I/O or background workers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Captures a lease under its exact owning UI scope, decoding at most 64 KiB
    /// for text. Invalid UTF-8 returns `preview_unsupported` with byte context;
    /// unknown MIME types return an `unsupported` descriptor without a URL.
    pub async fn create(&self, scope: &str, lease: ResourceLease) -> Result<PreviewDescriptor> {
        if lease.handle().scope != scope {
            return Err(Error::new(
                "scope_mismatch",
                "Preview scope must match the resource owner",
            ));
        }
        {
            let state = self.state.lock().map_err(|_| poisoned())?;
            active(&state, scope)?;
        }
        let metadata = lease.metadata();
        let mime = media_type(&metadata.mime, &metadata.name)?;
        let kind = classify(&mime);
        let id = format!("{}-{}", kind.token_prefix(), uuid::Uuid::new_v4());
        let mut descriptor = PreviewDescriptor {
            id: id.clone(),
            kind,
            name: metadata.name.clone(),
            mime: mime.clone(),
            text: None,
            truncated: None,
            url: None,
        };
        match kind {
            PreviewKind::Text => {
                let total = metadata.size.get();
                let bytes = read_slice(&lease, 0, total.min(TEXT_LIMIT)).await?;
                // A byte bound may split one UTF-8 scalar. Only a valid prefix
                // followed by an incomplete scalar at the cap may be trimmed.
                let (text, omitted) = match std::str::from_utf8(&bytes) {
                    Ok(text) => (text.to_owned(), false),
                    Err(error) if total > TEXT_LIMIT && error.error_len().is_none() => {
                        let prefix = std::str::from_utf8(&bytes[..error.valid_up_to()])
                            .map_err(|_| unsupported_text(error.valid_up_to()))?;
                        (prefix.to_owned(), true)
                    }
                    Err(error) => return Err(unsupported_text(error.valid_up_to())),
                };
                descriptor.text = Some(text);
                descriptor.truncated = Some(total > TEXT_LIMIT || omitted);
            }
            PreviewKind::Image | PreviewKind::Audio | PreviewKind::Video => {
                descriptor.url = Some(id.clone());
            }
            PreviewKind::Unsupported => {}
        }
        let mut state = self.state.lock().map_err(|_| poisoned())?;
        active(&state, scope)?;
        state.previews.insert(
            id,
            Preview {
                scope: scope.into(),
                lease,
                mime,
                kind,
            },
        );
        Ok(descriptor)
    }

    /// Releases a token only for its owning UI scope. Missing tokens are an
    /// idempotent success; a live token in another scope returns scope_mismatch.
    pub fn dispose(&self, scope: &str, id: &str) -> Result<()> {
        let removed = {
            let mut state = self.state.lock().map_err(|_| poisoned())?;
            if let Some(preview) = state.previews.get(id) {
                if preview.scope != scope {
                    return Err(Error::new(
                        "scope_mismatch",
                        "Preview belongs to another UI scope",
                    ));
                }
            }
            state.previews.remove(id)
        };
        // Final native resource cleanup must never run under the registry lock.
        drop(removed);
        Ok(())
    }

    /// Retires a UI scope, drops its retained leases outside the registry lock,
    /// and prevents pending/future creations. Repeated disposal is harmless.
    pub fn dispose_scope(&self, scope: &str) -> Result<()> {
        let removed = {
            let mut state = self.state.lock().map_err(|_| poisoned())?;
            state.disposed_scopes.insert(scope.into());
            let ids: Vec<_> = state
                .previews
                .iter()
                .filter(|(_, preview)| preview.scope == scope)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| state.previews.remove(&id))
                .collect::<Vec<_>>()
        };
        drop(removed);
        Ok(())
    }

    /// Reads a token capability, accepting no range or one `bytes=start-end`,
    /// `bytes=start-`, or `bytes=-suffix` range. Oversized valid ranges are capped
    /// at 4 MiB with an honest Content-Range. Invalid/unsatisfiable ranges return
    /// `preview_invalid_range` with status 416 and `bytes */total` in details;
    /// the host converts this structured error into its protocol response.
    pub async fn read(&self, token: &str, range: Option<&str>) -> Result<PreviewResponse> {
        let preview = {
            let state = self.state.lock().map_err(|_| poisoned())?;
            state.previews.get(token).cloned().ok_or_else(|| {
                Error::new("preview_not_found", "Preview token is missing or disposed")
            })?
        };
        if preview.kind == PreviewKind::Unsupported {
            return Err(Error::new(
                "preview_unsupported",
                "This resource has no supported preview",
            ));
        }
        let total = preview.lease.metadata().size.get();
        let (offset, length, partial) = interval(total, range)?;
        let body = read_slice(&preview.lease, offset, length).await?;
        let content_range =
            partial.then(|| format!("bytes {offset}-{}/{total}", offset + length - 1));
        Ok(PreviewResponse {
            body,
            mime: preview.mime,
            status: if partial { 206 } else { 200 },
            content_range,
            total,
        })
    }
}

fn active(state: &State, scope: &str) -> Result<()> {
    if state.disposed_scopes.contains(scope) {
        Err(Error::new(
            "scope_disposed",
            "Preview UI scope has been disposed",
        ))
    } else {
        Ok(())
    }
}

fn media_type(captured: &str, name: &str) -> Result<String> {
    let mime = if captured.trim().is_empty() {
        mime_guess::from_path(name)
            .first_or_octet_stream()
            .to_string()
    } else {
        captured.trim().to_owned()
    };
    if !mime.is_ascii()
        || mime.bytes().any(|b| b.is_ascii_control())
        || !mime.split(';').next().unwrap_or("").contains('/')
    {
        return Err(Error::new(
            "preview_invalid_mime",
            "Resource media type is invalid",
        ));
    }
    Ok(mime)
}

fn classify(mime: &str) -> PreviewKind {
    let essence = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if essence.starts_with("image/") {
        return PreviewKind::Image;
    }
    if essence.starts_with("text/")
        || matches!(
            essence.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
        )
        || essence.ends_with("+json")
        || essence.ends_with("+xml")
    {
        PreviewKind::Text
    } else if essence.starts_with("audio/") {
        PreviewKind::Audio
    } else if essence.starts_with("video/") {
        PreviewKind::Video
    } else {
        PreviewKind::Unsupported
    }
}

async fn read_slice(lease: &ResourceLease, offset: u64, length: u64) -> Result<Vec<u8>> {
    debug_assert!(length <= RESPONSE_LIMIT);
    let mut bytes = Vec::with_capacity(length as usize);
    while (bytes.len() as u64) < length {
        let remaining = length - bytes.len() as u64;
        let chunk = lease
            .read(offset + bytes.len() as u64, remaining as u32)
            .await?;
        if chunk.is_empty() {
            return Err(Error::new(
                "preview_unexpected_eof",
                "Preview ended before its captured size",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn interval(total: u64, range: Option<&str>) -> Result<(u64, u64, bool)> {
    let Some(range) = range else {
        return Ok((0, total.min(RESPONSE_LIMIT), total > RESPONSE_LIMIT));
    };
    let invalid = || {
        Error::new("preview_invalid_range", "Expected a satisfiable single byte range")
        .details(json!({"status": 416, "contentRange": format!("bytes */{total}"), "total": total.to_string()}))
    };
    let specification = range.trim().strip_prefix("bytes=").ok_or_else(invalid)?;
    let (start, end) = specification.split_once('-').ok_or_else(invalid)?;
    let number = |text: &str| -> Result<u64> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        text.parse().map_err(|_| invalid())
    };
    if total == 0 {
        return Err(invalid());
    }
    let (offset, end) = if start.is_empty() {
        let suffix = number(end)?;
        if suffix == 0 {
            return Err(invalid());
        }
        (total.saturating_sub(suffix), total - 1)
    } else {
        let offset = number(start)?;
        let end = if end.is_empty() {
            total - 1
        } else {
            number(end)?
        };
        if offset >= total || end < offset {
            return Err(invalid());
        }
        (offset, end.min(total - 1))
    };
    Ok((offset, (end - offset + 1).min(RESPONSE_LIMIT), true))
}

fn unsupported_text(offset: usize) -> Error {
    Error::new("preview_unsupported", "Text preview requires valid UTF-8")
        .details(json!({"reason": "invalid_utf8", "byteOffset": offset}))
}

fn poisoned() -> Error {
    Error::new(
        "preview_unavailable",
        "Preview synchronization was poisoned",
    )
}
