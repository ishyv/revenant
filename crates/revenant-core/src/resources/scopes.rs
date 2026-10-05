//! Disposal invalidates future admission in a scope and its descendants. Disposed scope tombstones prevent reuse of scope names and resurrection of old tasks.
use super::*;
pub(super) struct Scope {
    pub(super) parent: Option<String>,
    pub(super) active: bool,
}
impl ResourceRegistry {
    /// Creates an active named scope, optionally beneath an active parent.
    /// Empty or previously used identifiers return `invalid_scope`; inactive or
    /// unknown parents return `scope_disposed`. Disposed IDs cannot be reused.
    pub fn create_scope(&self, id: &str, parent: Option<&str>) -> Result<()> {
        let mut inner = lock(&self.inner);
        if id.is_empty() || inner.scopes.contains_key(id) {
            return Err(Error::new(
                "invalid_scope",
                "Scope id is empty or has already been used",
            ));
        }
        if let Some(parent) = parent {
            Self::active(&inner, parent)?;
        }
        inner.scopes.insert(
            id.into(),
            Scope {
                parent: parent.map(str::to_owned),
                active: true,
            },
        );
        Ok(())
    }
    pub(super) fn active(inner: &Resources, scope: &str) -> Result<()> {
        if !inner.scopes.get(scope).is_some_and(|s| s.active) {
            return Err(Error::new("scope_disposed", "Scope is missing or disposed"));
        }
        Ok(())
    }
    /// Returns true only for a known scope that has not been disposed.
    pub fn is_scope_active(&self, scope: &str) -> bool {
        Self::active(&lock(&self.inner), scope).is_ok()
    }
    /// Disposes this scope and every descendant, releasing their registry owners.
    /// Leases retain read authority and delay final cleanup. Repeated disposal of
    /// a known scope succeeds; an unknown scope returns `scope_missing`. Scope
    /// tombstones remain to prevent old task contexts from becoming active again.
    pub fn dispose_scope(&self, scope: &str) -> Result<()> {
        let removed = {
            let mut inner = lock(&self.inner);
            if !inner.scopes.contains_key(scope) {
                return Err(Error::new("scope_missing", "Unknown scope"));
            }
            let mut disposed = vec![scope.to_owned()];
            let mut cursor = 0;
            while cursor < disposed.len() {
                let parent = &disposed[cursor];
                let children: Vec<_> = inner
                    .scopes
                    .iter()
                    .filter(|(_, s)| s.parent.as_ref() == Some(parent))
                    .map(|(id, _)| id.clone())
                    .collect();
                disposed.extend(children);
                cursor += 1;
            }
            for id in &disposed {
                inner.scopes.get_mut(id).unwrap().active = false;
            }
            let ids: Vec<_> = inner
                .resources
                .iter()
                .filter(|(_, r)| disposed.contains(&r.handle.scope))
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| inner.resources.remove(&id))
                .collect::<Vec<_>>()
        };
        drop(removed);
        Ok(())
    }
}
