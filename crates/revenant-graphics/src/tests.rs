use super::*;

fn primitive(key: &str, x: f32, size: f32) -> Primitive {
    Primitive {
        key: key.into(),
        shape: Shape::Rectangle,
        bounds: [x, 0.0, size, size],
        color: [0.2, 0.6, 0.9, 1.0],
        label: key.into(),
    }
}

#[test]
fn replaces_scene_atomically_and_rejects_invalid_geometry() {
    let scene = Scene2D::new();
    scene.replace(vec![primitive("a", 0.0, 10.0)]).unwrap();
    let first = scene.snapshot();
    assert_eq!(first.primitives.len(), 1);
    assert!(
        scene
            .replace(vec![primitive("bad", f32::NAN, 2.0)])
            .is_err()
    );
    assert_eq!(scene.snapshot().revision, first.revision);
    assert_eq!(scene.snapshot().primitives[0].key, "a");
}

#[test]
fn bounded_scene_rejects_duplicate_keys_and_excess_geometry() {
    let scene = Scene2D::new();
    assert!(
        scene
            .replace(vec![primitive("a", 0.0, 1.0), primitive("a", 2.0, 1.0)])
            .is_err()
    );
    assert!(
        scene
            .replace(
                (0..=MAX_PRIMITIVES)
                    .map(|i| primitive(&i.to_string(), 0.0, 1.0))
                    .collect()
            )
            .is_err()
    );
    assert!(scene.snapshot().primitives.is_empty());
}

#[test]
fn picking_uses_shape_boundaries_and_stable_scene_keys() {
    let scene = Scene2D::new();
    let mut circle = primitive("circle", 0.0, 10.0);
    circle.shape = Shape::Circle;
    scene.replace(vec![circle]).unwrap();
    assert_eq!(scene.pick([5.0, 5.0]).as_deref(), Some("circle"));
    assert!(scene.pick([0.0, 0.0]).is_none());
    scene
        .replace(vec![primitive("replacement", 20.0, 5.0)])
        .unwrap();
    assert!(scene.pick([5.0, 5.0]).is_none());
}

#[test]
fn old_snapshots_remain_immutable_when_scene_changes() {
    let scene = Scene2D::new();
    scene.replace(vec![primitive("first", 0.0, 1.0)]).unwrap();
    let old = scene.snapshot();
    scene.replace(vec![primitive("second", 1.0, 2.0)]).unwrap();
    assert_eq!(old.primitives[0].key, "first");
    assert!(scene.snapshot().revision > old.revision);
}

#[test]
fn patch_is_atomic_and_preserves_order_and_snapshots() {
    let scene = Scene2D::new();
    let revision = scene
        .replace(vec![primitive("a", 0., 10.), primitive("b", 1., 10.)])
        .unwrap();
    let old = scene.snapshot();
    let next = scene
        .patch(
            revision,
            vec![
                SceneChange::Update(primitive("a", 5., 10.)),
                SceneChange::Remove("b".into()),
                SceneChange::Add(primitive("c", 2., 10.)),
            ],
        )
        .unwrap();
    assert_eq!(next, revision + 1);
    assert_eq!(
        scene
            .snapshot()
            .primitives
            .iter()
            .map(|p| p.key.as_str())
            .collect::<Vec<_>>(),
        ["a", "c"]
    );
    assert_eq!(old.primitives[0].bounds[0], 0.);
    let committed = scene.snapshot();
    for changes in [
        vec![SceneChange::Add(primitive("a", 0., 1.))],
        vec![SceneChange::Remove("missing".into())],
        vec![SceneChange::Update(primitive("missing", 0., 1.))],
        vec![SceneChange::Update(primitive("a", f32::NAN, 1.))],
        vec![
            SceneChange::Remove("a".into()),
            SceneChange::Add(primitive("a", 0., 1.)),
        ],
    ] {
        assert!(scene.patch(next, changes).is_err());
        assert!(Arc::ptr_eq(&scene.snapshot(), &committed));
    }
    assert!(scene.patch(revision, vec![]).is_err());
    assert!(Arc::ptr_eq(&scene.snapshot(), &committed));
}

#[test]
fn patch_enforces_capacity_revision_and_wakes_outside_lock() {
    let scene = Scene2D::new();
    let revision = scene
        .replace(
            (0..MAX_PRIMITIVES)
                .map(|i| primitive(&i.to_string(), 0., 1.))
                .collect(),
        )
        .unwrap();
    assert!(
        scene
            .patch(
                revision,
                vec![SceneChange::Add(primitive("overflow", 0., 1.))]
            )
            .is_err()
    );
    let observed = scene.clone();
    let _subscription = scene.subscribe(Arc::new(move || {
        let _ = observed.snapshot();
    }));
    let next = scene
        .patch(
            revision,
            vec![
                SceneChange::Remove("0".into()),
                SceneChange::Add(primitive("replacement", 0., 1.)),
            ],
        )
        .unwrap();
    scene.0.lock().unwrap().frame = Arc::new(SceneFrame {
        revision: u64::MAX,
        primitives: vec![],
    });
    assert!(
        scene
            .patch(u64::MAX, vec![SceneChange::Add(primitive("a", 0., 1.))])
            .is_err()
    );
    assert!(next > revision);
    assert_eq!(scene.snapshot().revision, u64::MAX);
}
