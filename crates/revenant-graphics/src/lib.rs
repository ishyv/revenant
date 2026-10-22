//! Bounded native scene ownership, independent of the desktop transport.
#![deny(missing_docs)]

use revenant_core::{Error, Result};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

/// Maximum primitives retained by a scene.
pub const MAX_PRIMITIVES: usize = 50_000;

/// Geometry supported by the native renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Axis-aligned rectangle.
    Rectangle,
    /// Ellipse inscribed in the primitive bounds.
    Circle,
}

/// Native-only geometry. Paths and inventories never cross the graphics transport.
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    /// Stable selection identity, independent of layout order.
    pub key: String,
    /// Shape drawn inside the bounds.
    pub shape: Shape,
    /// World coordinates: x, y, width, height.
    pub bounds: [f32; 4],
    /// Linear RGBA components in the inclusive range 0..1.
    pub color: [f32; 4],
    /// Bounded visible label.
    pub label: String,
}

/// An explicit keyed edit. Each key may occur only once in a patch.
#[derive(Clone, Debug)]
pub enum SceneChange {
    /// Append a primitive whose key is absent from the base frame.
    Add(Primitive),
    /// Replace a present primitive while retaining its painting position.
    Update(Primitive),
    /// Remove a present key.
    Remove(String),
}

/// Immutable published scene frame; old snapshots remain valid across replacement.
#[derive(Clone, Debug)]
pub struct SceneFrame {
    /// Monotonic content revision.
    pub revision: u64,
    /// Geometry in painting order.
    pub primitives: Vec<Primitive>,
}

struct SceneState {
    frame: Arc<SceneFrame>,
    positions: HashMap<String, usize>,
    listeners: Vec<(u64, Arc<dyn Fn() + Send + Sync>)>,
    next_listener: u64,
}

/// Cloneable native scene resource, with atomic bounded frame replacement.
#[derive(Clone)]
pub struct Scene2D(Arc<Mutex<SceneState>>);

impl Default for Scene2D {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene2D {
    /// Creates an empty scene.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(SceneState {
            frame: Arc::new(SceneFrame {
                revision: 0,
                primitives: Vec::new(),
            }),
            positions: HashMap::new(),
            listeners: Vec::new(),
            next_listener: 0,
        })))
    }

    /// Validates all geometry before replacing the current frame.
    pub fn replace(&self, primitives: Vec<Primitive>) -> Result<u64> {
        if primitives.len() > MAX_PRIMITIVES {
            return Err(Error::new(
                "scene_limit",
                "A scene supports at most 50,000 primitives",
            ));
        }
        let mut keys = HashSet::with_capacity(primitives.len());
        for p in &primitives {
            validate_primitive(p)?;
            if !keys.insert(p.key.as_str()) {
                return Err(Error::new(
                    "invalid_scene",
                    "Scene keys, labels, colors, and geometry must be bounded and valid",
                ));
            }
        }
        let (revision, listeners) = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let revision = state
                .frame
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::new("scene_revision", "Scene revision exhausted"))?;
            state.positions = primitives
                .iter()
                .enumerate()
                .map(|(i, p)| (p.key.clone(), i))
                .collect();
            state.frame = Arc::new(SceneFrame {
                revision,
                primitives,
            });
            (
                revision,
                state
                    .listeners
                    .iter()
                    .map(|(_, f)| f.clone())
                    .collect::<Vec<_>>(),
            )
        };
        // Never call a host wake-up callback while holding the scene lock.
        for listener in listeners {
            listener();
        }
        Ok(revision)
    }

    /// Atomically applies edits to exactly `expected_revision`.
    /// Stale revisions, duplicate edits, missing updates/removals, existing additions,
    /// and invalid geometry leave the frame and its revision unchanged.
    /// Snapshots remain immutable; publication currently copies the base primitive vector.
    pub fn patch(&self, expected_revision: u64, changes: Vec<SceneChange>) -> Result<u64> {
        if changes.len() > MAX_PRIMITIVES {
            return Err(Error::new(
                "scene_limit",
                "A patch supports at most 50,000 edits",
            ));
        }
        let (revision, listeners) = {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.frame.revision != expected_revision {
                return Err(Error::new("scene_revision", "Patch base revision is stale"));
            }
            let revision = expected_revision
                .checked_add(1)
                .ok_or_else(|| Error::new("scene_revision", "Scene revision exhausted"))?;
            let mut edited = HashSet::with_capacity(changes.len());
            let mut count = state.frame.primitives.len();
            for change in &changes {
                let key = match change {
                    SceneChange::Add(p) | SceneChange::Update(p) => {
                        validate_primitive(p)?;
                        &p.key
                    }
                    SceneChange::Remove(key) => key,
                };
                if key.is_empty() || key.len() > 256 || !edited.insert(key.as_str()) {
                    return Err(Error::new(
                        "invalid_scene",
                        "Patch keys must be bounded and unique",
                    ));
                }
                let exists = state.positions.contains_key(key);
                match change {
                    SceneChange::Add(_) if !exists => count += 1,
                    SceneChange::Update(_) if exists => {}
                    SceneChange::Remove(_) if exists => count -= 1,
                    _ => {
                        return Err(Error::new(
                            "invalid_scene",
                            "Patch key does not match its edit kind",
                        ));
                    }
                }
            }
            if count > MAX_PRIMITIVES {
                return Err(Error::new(
                    "scene_limit",
                    "A scene supports at most 50,000 primitives",
                ));
            }
            let mut primitives = state.frame.primitives.clone();
            let structural = changes.iter().any(|c| !matches!(c, SceneChange::Update(_)));
            let mut removed = HashSet::new();
            let mut added = Vec::new();
            for change in changes {
                match change {
                    SceneChange::Update(p) => {
                        let index = state.positions[&p.key];
                        primitives[index] = p;
                    }
                    SceneChange::Remove(key) => {
                        removed.insert(key);
                    }
                    SceneChange::Add(p) => added.push(p),
                }
            }
            if !removed.is_empty() {
                primitives.retain(|p| !removed.contains(&p.key));
            }
            primitives.extend(added);
            if structural {
                state.positions = primitives
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (p.key.clone(), i))
                    .collect();
            }
            state.frame = Arc::new(SceneFrame {
                revision,
                primitives,
            });
            (
                revision,
                state
                    .listeners
                    .iter()
                    .map(|(_, f)| f.clone())
                    .collect::<Vec<_>>(),
            )
        };
        for listener in listeners {
            listener();
        }
        Ok(revision)
    }

    /// Reads the latest immutable frame without copying its geometry.
    pub fn snapshot(&self) -> Arc<SceneFrame> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .frame
            .clone()
    }

    /// Picks the topmost primitive at a world-space point.
    pub fn pick(&self, point: [f32; 2]) -> Option<String> {
        self.snapshot()
            .primitives
            .iter()
            .rev()
            .find(|p| contains(p, point))
            .map(|p| p.key.clone())
    }

    /// Registers a host wake-up callback. Dropping the subscription unregisters it.
    pub fn subscribe(&self, callback: Arc<dyn Fn() + Send + Sync>) -> SceneSubscription {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.next_listener += 1;
        let id = state.next_listener;
        state.listeners.push((id, callback));
        SceneSubscription {
            scene: self.clone(),
            id,
        }
    }
}

fn validate_primitive(p: &Primitive) -> Result<()> {
    if p.key.is_empty()
        || p.key.len() > 256
        || p.label.len() > 512
        || p.bounds.iter().any(|v| !v.is_finite() || v.abs() > 1e8)
        || p.bounds[2] < 0.0
        || p.bounds[3] < 0.0
        || p.color
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err(Error::new(
            "invalid_scene",
            "Scene keys, labels, colors, and geometry must be bounded and valid",
        ));
    }
    Ok(())
}

/// Registration whose lifetime controls host scene notifications.
pub struct SceneSubscription {
    scene: Scene2D,
    id: u64,
}
impl Drop for SceneSubscription {
    fn drop(&mut self) {
        self.scene
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .listeners
            .retain(|(id, _)| *id != self.id);
    }
}

pub(crate) fn contains(p: &Primitive, point: [f32; 2]) -> bool {
    contains_bounds(p.shape, p.bounds, point)
}

pub(crate) fn contains_bounds(shape: Shape, bounds: [f32; 4], point: [f32; 2]) -> bool {
    let [x, y, w, h] = bounds;
    if w == 0.0 || h == 0.0 {
        return false;
    }
    let u = (point[0] - x) / w;
    let v = (point[1] - y) / h;
    (0.0..=1.0).contains(&u)
        && (0.0..=1.0).contains(&v)
        && (shape == Shape::Rectangle || (u * 2.0 - 1.0).powi(2) + (v * 2.0 - 1.0).powi(2) <= 1.0)
}

#[cfg(all(feature = "gpu", windows))]
mod compiler;
#[cfg(feature = "experimental-compute")]
pub mod compute;
#[cfg(feature = "gpu")]
pub mod gpu;

#[cfg(test)]
mod tests;
