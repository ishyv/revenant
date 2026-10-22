//! Host-issued identities outlive active records. Disposal visits indexed active descendants.
use super::*;
use std::collections::HashSet;
pub(super) struct Scope {
    pub(super) parent: Option<String>,
    children: HashSet<String>,
}

impl ResourceRegistry {
    /// Issues an opaque identity, optionally beneath an active parent.
    /// IDs combine a registry instance with a never-reused sequence. Inactive or
    /// foreign parents return `scope_disposed`; exhaustion returns `resource_exhausted`.
    pub fn create_scope(&self, parent: Option<&str>) -> Result<String> {
        let mut inner = lock(&self.inner);
        if let Some(parent) = parent {
            Self::active(&inner, parent)?;
        }
        let sequence = inner
            .issued_scope
            .checked_add(1)
            .ok_or_else(|| Error::new("resource_exhausted", "Scope identity sequence exhausted"))?;
        let id = format!("scope:{}:{sequence}", inner.scope_instance);
        inner.scopes.insert(
            id.clone(),
            Scope {
                parent: parent.map(str::to_owned),
                children: HashSet::new(),
            },
        );
        if let Some(parent) = parent {
            inner
                .scopes
                .get_mut(parent)
                .unwrap()
                .children
                .insert(id.clone());
        }
        inner.issued_scope = sequence;
        Ok(id)
    }
    fn issued(inner: &Resources, scope: &str) -> bool {
        let Some((instance, sequence)) = scope.rsplit_once(':') else {
            return false;
        };
        let Ok(sequence) = sequence.parse::<u64>() else {
            return false;
        };
        sequence > 0
            && sequence <= inner.issued_scope
            && instance == format!("scope:{}", inner.scope_instance)
            && scope == format!("{instance}:{sequence}")
    }
    pub(super) fn active(inner: &Resources, scope: &str) -> Result<()> {
        if !inner.scopes.contains_key(scope) {
            return Err(Error::new("scope_disposed", "Scope is missing or disposed"));
        }
        Ok(())
    }
    /// Returns true only for a known scope that has not been disposed.
    pub fn is_scope_active(&self, scope: &str) -> bool {
        Self::active(&lock(&self.inner), scope).is_ok()
    }
    /// Number of active scope records. Retired identities retain no records.
    pub fn active_scope_count(&self) -> usize {
        lock(&self.inner).scopes.len()
    }
    /// Disposes this scope and every descendant, releasing their registry owners.
    /// Leases retain read authority and delay final cleanup. Repeated disposal of
    /// an issued retired identity succeeds. Malformed, foreign or unissued IDs
    /// return `scope_missing`. Only active scope records are retained.
    pub fn dispose_scope(&self, scope: &str) -> Result<()> {
        let removed = {
            let mut inner = lock(&self.inner);
            if !Self::issued(&inner, scope) {
                return Err(Error::new(
                    "scope_missing",
                    "Scope identity was not issued by this registry",
                ));
            }
            let Some(root) = inner.scopes.remove(scope) else {
                return Ok(());
            };
            if let Some(parent) = root.parent {
                inner
                    .scopes
                    .get_mut(&parent)
                    .unwrap()
                    .children
                    .remove(scope);
            }
            let mut pending = Vec::from_iter(root.children);
            let mut disposed = HashSet::from([scope.to_owned()]);
            while let Some(id) = pending.pop() {
                let entry = inner.scopes.remove(&id).unwrap();
                pending.extend(entry.children);
                disposed.insert(id);
            }
            let ids: Vec<_> = inner
                .resources
                .iter()
                .filter(|(_, r)| disposed.contains(&r.handle.scope))
                .map(|(id, _)| id.clone())
                .collect();
            let files = ids
                .into_iter()
                .filter_map(|id| inner.resources.remove(&id))
                .collect::<Vec<_>>();
            let native_ids: Vec<_> = inner
                .native_resources
                .iter()
                .filter(|(_, r)| disposed.contains(&r.handle.scope))
                .map(|(id, _)| id.clone())
                .collect();
            let native = native_ids
                .into_iter()
                .filter_map(|id| inner.native_resources.remove(&id))
                .collect::<Vec<_>>();
            if inner.scopes.capacity() > inner.scopes.len().saturating_mul(4).max(64) {
                let retained = inner.scopes.len().max(64);
                inner.scopes.shrink_to(retained);
            }
            (files, native)
        };
        drop(removed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_exhaustion_fails_closed_and_does_not_reuse_retired_identity() {
        let registry = ResourceRegistry::new();
        lock(&registry.inner).issued_scope = u64::MAX - 1;
        let last = registry.create_scope(None).unwrap();
        assert!(last.ends_with(&u64::MAX.to_string()));
        registry.dispose_scope(&last).unwrap();
        assert_eq!(
            registry.create_scope(None).unwrap_err().code,
            "resource_exhausted"
        );
        registry.dispose_scope(&last).unwrap();
        assert_eq!(registry.active_scope_count(), 0);
    }

    #[test]
    fn disposed_scope_records_are_reclaimed_after_extended_churn() {
        let registry = ResourceRegistry::new();
        for _ in 0..100_000 {
            let scope = registry.create_scope(None).unwrap();
            registry.dispose_scope(&scope).unwrap();
        }
        let inner = lock(&registry.inner);
        assert!(
            inner.scopes.is_empty(),
            "disposed identity history must not retain scope records"
        );
        assert!(inner.resources.is_empty());
        assert!(inner.native_resources.is_empty());
    }
}
