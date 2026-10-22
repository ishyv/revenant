//! UI ownership scopes bind native references to one actual webview.
//!
//! Opaque IDs identify resources, but never replace admission checks. Every IPC
//! request is checked against the calling window before it reaches the runtime.

use super::{DesktopRuntime, Error, Result, lock};
use std::collections::HashSet;

pub(crate) struct ScopeRecord {
    pub parent: Option<String>,
    pub owner: ScopeOwner,
    children: HashSet<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ScopeOwner {
    Window(String),
    Agent(String),
}

impl DesktopRuntime {
    /// Creates a root UI scope bound to an actual platform window label.
    pub fn connect(&self, window: &str) -> Result<serde_json::Value> {
        self.accepting()?;
        let scope = self.create_scope_inner(None, ScopeOwner::Window(window.into()))?;
        Ok(
            serde_json::json!({"scope":scope,"manifest":self.inner.registry.manifest()?,"development":self.inner.options.development,"instanceId":self.inner.instance_id}),
        )
    }

    pub(crate) fn create_scope_inner(
        &self,
        parent: Option<&str>,
        owner: ScopeOwner,
    ) -> Result<String> {
        let mut scopes = lock(&self.inner.scopes);
        if scopes.len() >= self.inner.options.max_scopes {
            return Err(Error::new(
                "scope_limit",
                "Too many active UI ownership scopes",
            ));
        }
        if let Some(parent) = parent
            && !scopes.contains_key(parent)
        {
            return Err(Error::new(
                "scope_disposed",
                "UI ownership scope has been disposed",
            ));
        }
        let scope = self.inner.resources.create_scope(parent)?;
        scopes.insert(
            scope.clone(),
            ScopeRecord {
                parent: parent.map(str::to_owned),
                owner,
                children: HashSet::new(),
            },
        );
        if let Some(parent) = parent {
            scopes
                .get_mut(parent)
                .unwrap()
                .children
                .insert(scope.clone());
        }
        Ok(scope)
    }

    /// Creates a child scope whose disposal releases its own views and operations.
    pub fn create_scope(&self, parent: &str) -> Result<String> {
        self.accepting()?;
        let owner = self.scope_owner(parent)?;
        self.create_scope_inner(Some(parent), owner)
    }

    pub(crate) fn scope_owner(&self, scope: &str) -> Result<ScopeOwner> {
        lock(&self.inner.scopes)
            .get(scope)
            .map(|record| record.owner.clone())
            .ok_or_else(|| Error::new("scope_disposed", "UI ownership scope has been disposed"))
    }

    /// Rejects requests that present a scope belonging to another window.
    pub fn validate_window(&self, scope: &str, window: &str) -> Result<()> {
        if self.scope_owner(scope)? != ScopeOwner::Window(window.into()) {
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
            let mut scopes = lock(&self.inner.scopes);
            if !scopes.contains_key(scope) {
                return self.inner.resources.dispose_scope(scope);
            }
            let mut ids = vec![scope.to_owned()];
            let mut index = 0;
            while index < ids.len() {
                let children: Vec<_> = scopes[&ids[index]].children.iter().cloned().collect();
                ids.extend(children);
                index += 1;
            }
            for id in &ids {
                if let Some(record) = scopes.remove(id)
                    && let Some(parent) = record.parent
                    && let Some(parent) = scopes.get_mut(&parent)
                {
                    parent.children.remove(id);
                }
            }
            ids
        };
        // Retire authority before preview cleanup: an admitted read may finish,
        // but no pending creation can publish after its scope has closed.
        // Complete independent teardown stages before propagating an observer
        // unwind. Removed scope identities cannot be used to retry skipped work.
        let mut panic = None;
        let mut error = None;
        let mut finish = |action: &mut dyn FnMut() -> Result<()>| match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(action),
        ) {
            Ok(Err(failure)) => {
                if error.is_none() {
                    error = Some(failure);
                }
            }
            Err(payload) => {
                if panic.is_none() {
                    panic = Some(payload);
                }
            }
            Ok(Ok(())) => {}
        };
        finish(&mut || self.inner.resources.dispose_scope(scope));
        for id in &disposed {
            finish(&mut || {
                self.cancel_scope_jobs(id);
                Ok(())
            });
            lock(&self.inner.selections).retain(|_, selection| selection.scope != *id);
            let views: Vec<_> = lock(&self.inner.views)
                .values()
                .filter(|view| view.scope == *id)
                .map(|view| view.id.clone())
                .collect();
            for view in views {
                finish(&mut || self.dispose_view(id, &view));
            }
            lock(&self.inner.folders).retain(|_, folder| folder.scope != *id);
            finish(&mut || self.inner.previews.dispose_scope(id));
        }
        finish(&mut || self.inner.tasks.dispose_scope(scope));
        drop(finish);
        if let Some(payload) = panic {
            std::panic::resume_unwind(payload);
        }
        error.map_or(Ok(()), Err)
    }
}
