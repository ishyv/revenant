//! UI ownership scopes bind native references to one actual webview.
//!
//! Opaque IDs identify resources, but never replace admission checks. Every IPC
//! request is checked against the calling window before it reaches the runtime.

use super::{DesktopRuntime, Error, Result, id, lock};

pub(crate) struct ScopeRecord {
    pub parent: Option<String>,
    pub window: String,
}

impl DesktopRuntime {
    /// Creates a root UI scope bound to an actual platform window label.
    pub fn connect(&self, window: &str) -> Result<serde_json::Value> {
        self.accepting()?;
        let scope = self.create_scope_inner(None, window)?;
        Ok(serde_json::json!({"scope":scope,"manifest":self.inner.registry.manifest()?}))
    }

    fn create_scope_inner(&self, parent: Option<&str>, window: &str) -> Result<String> {
        let mut scopes = lock(&self.inner.scopes);
        if scopes.len() >= self.inner.options.max_scopes {
            return Err(Error::new(
                "scope_limit",
                "Too many active UI ownership scopes",
            ));
        }
        let scope = id("scope");
        self.inner.resources.create_scope(&scope, parent)?;
        scopes.insert(
            scope.clone(),
            ScopeRecord {
                parent: parent.map(str::to_owned),
                window: window.into(),
            },
        );
        Ok(scope)
    }

    /// Creates a child scope whose disposal releases its own views and operations.
    pub fn create_scope(&self, parent: &str) -> Result<String> {
        self.accepting()?;
        let window = self.scope_window(parent)?;
        self.create_scope_inner(Some(parent), &window)
    }

    pub(crate) fn scope_window(&self, scope: &str) -> Result<String> {
        lock(&self.inner.scopes)
            .get(scope)
            .map(|record| record.window.clone())
            .ok_or_else(|| Error::new("scope_disposed", "UI ownership scope has been disposed"))
    }

    /// Rejects requests that present a scope belonging to another window.
    pub fn validate_window(&self, scope: &str, window: &str) -> Result<()> {
        if self.scope_window(scope)? != window {
            return Err(Error::new(
                "scope_mismatch",
                "Native references belong to another window",
            ));
        }
        Ok(())
    }

    /// Cancels scoped tasks and releases native views, previews, and folder ownership.
    ///
    /// Tasks already holding input leases retain those resources until cancellation
    /// is acknowledged; scope disposal never frees an executing reader underneath it.
    pub fn dispose_scope(&self, scope: &str) -> Result<()> {
        let disposed = {
            let scopes = lock(&self.inner.scopes);
            if !scopes.contains_key(scope) {
                return Ok(());
            }
            let mut ids = vec![scope.to_owned()];
            let mut index = 0;
            while index < ids.len() {
                let children: Vec<_> = scopes
                    .iter()
                    .filter(|(_, s)| s.parent.as_deref() == Some(ids[index].as_str()))
                    .map(|(id, _)| id.clone())
                    .collect();
                ids.extend(children);
                index += 1;
            }
            ids
        };
        for id in &disposed {
            self.cancel_scope_jobs(id);
            lock(&self.inner.selections).retain(|_, selection| selection.scope != *id);
            let views: Vec<_> = lock(&self.inner.views)
                .values()
                .filter(|view| view.scope == *id)
                .map(|view| view.id.clone())
                .collect();
            for view in views {
                let _ = self.dispose_view(id, &view);
            }
            lock(&self.inner.folders).retain(|_, folder| folder.scope != *id);
            self.inner.previews.dispose_scope(id)?;
            lock(&self.inner.scopes).remove(id);
        }
        self.inner.tasks.dispose_scope(scope)?;
        Ok(())
    }
}
