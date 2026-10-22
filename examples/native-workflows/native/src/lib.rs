//! Non-disk workflow fixture: bulk models remain native and reads return projections.
use revenant::{
    DecimalU64, Error, LatestJobs, Resource, Result, TaskContext, WireProjection, WorkClass,
    contract, operation,
};
use revenant_graphics::{Primitive, Scene2D, SceneChange, Shape};
use std::sync::{Arc, Mutex};
revenant::app!();

#[derive(WireProjection)]
#[wire(id)]
pub struct FixtureId(pub String);
#[derive(WireProjection)]
pub struct Details {
    pub seeded: bool,
    pub host_paths_available: bool,
}
#[derive(WireProjection)]
pub struct Summary {
    pub id: FixtureId,
    pub entries: u32,
    #[wire(decimal)]
    pub checksum: u64,
    pub details: Option<Details>,
}
pub struct Dataset {
    entries: u32,
    checksum: u64,
    host_paths: bool,
}
pub struct SceneState {
    version: u64,
    shift: u32,
    scene: Scene2D,
}
pub struct SceneController {
    state: Arc<Mutex<SceneState>>,
    jobs: LatestJobs<SceneState, u32, Vec<SceneChange>, u64>,
}
#[contract]
pub struct SceneHandles {
    pub scene: Resource<Scene2D>,
    pub controller: Resource<SceneController>,
}

fn transform(index: u32) -> u32 {
    let mut value = index.wrapping_add(378);
    for _ in 0..32 {
        value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
    value
}
/// Generate deterministic native data with bounded chunked map/reduce.
#[operation(id = "fixture.build")]
pub fn build(entries: u32, context: TaskContext) -> Result<Resource<Dataset>> {
    if !(1..=1_000_000).contains(&entries) {
        return Err(Error::new("fixture_limit", "Choose 1..=1,000,000 entries"));
    }
    let checksum = context.work()?.map_reduce(
        WorkClass::Cpu,
        0..entries.div_ceil(1024),
        move |chunk, token| {
            token.checkpoint()?;
            Ok((chunk * 1024..((chunk + 1) * 1024).min(entries))
                .map(|index| u64::from(transform(index)))
                .sum::<u64>())
        },
        0u64,
        |sum, value| {
            *sum += value;
            Ok(())
        },
    )?;
    let host_paths = context.application_paths().is_ok();
    context.own(Dataset {
        entries,
        checksum,
        host_paths,
    })
}
/// Read a generated projection without serializing the native dataset.
#[operation(id = "fixture.summary", read_only, work = "interactive", agent)]
pub fn summary(dataset: Resource<Dataset>, context: TaskContext) -> Result<SummaryWire> {
    let data = context.lease_resource(&dataset)?;
    Ok(Summary {
        id: FixtureId("seed-378".into()),
        entries: data.entries,
        checksum: data.checksum,
        details: Some(Details {
            seeded: true,
            host_paths_available: data.host_paths,
        }),
    }
    .into_wire())
}
/// Exercise an optional grouped-read failure without contaminating retained data.
#[operation(id = "fixture.optional", read_only, work = "interactive")]
pub fn optional(fail: bool) -> Result<bool> {
    if fail {
        Err(Error::new(
            "fixture_optional",
            "Requested optional-slot failure",
        ))
    } else {
        Ok(true)
    }
}
/// Small explicitly exposed agent read; every reply stays independent of bulk buffers.
#[operation(id = "fixture.agentSummary", read_only, work = "interactive", agent)]
pub fn agent_summary(context: TaskContext) -> Result<SummaryWire> {
    Ok(Summary {
        id: FixtureId("seed-378".into()),
        entries: 0,
        checksum: u64::MAX,
        details: Some(Details {
            seeded: true,
            host_paths_available: context.application_paths().is_ok(),
        }),
    }
    .into_wire())
}
fn primitive(index: u32, shift: f32) -> Primitive {
    Primitive {
        key: format!("key-{index}"),
        shape: Shape::Rectangle,
        bounds: [
            (index % 250) as f32 * 4.0 + shift,
            (index / 250) as f32 * 4.0,
            3.0,
            3.0,
        ],
        color: [0.2, 0.65, 0.85, 1.0],
        label: String::new(),
    }
}
/// Own a complete 50,000-key scene with an independent native surface lifetime.
#[operation(id = "fixture.scene")]
pub fn scene(context: TaskContext) -> Result<SceneHandles> {
    let scene = Scene2D::new();
    scene.replace((0..50_000).map(|index| primitive(index, 0.0)).collect())?;
    let state = Arc::new(Mutex::new(SceneState {
        version: 0,
        shift: 0,
        scene: scene.clone(),
    }));
    let jobs = LatestJobs::new(
        context.clone(),
        state.clone(),
        1,
        |shift, context| {
            context
                .work()?
                .map_ordered(WorkClass::Cpu, 0..500, move |index, token| {
                    token.checkpoint()?;
                    Ok(SceneChange::Update(primitive(index, shift as f32)))
                })
        },
        |state| state.version,
        |state, changes| {
            let revision = state.scene.snapshot().revision;
            state.scene.patch(revision, changes)?;
            Ok(())
        },
    )?;
    Ok(SceneHandles {
        scene: context.own(scene)?,
        controller: context.own(SceneController { state, jobs })?,
    })
}
/// Capture a view version, compute outside the application lock and commit a 1% patch.
#[operation(id = "fixture.patch")]
pub fn patch(
    controller: Resource<SceneController>,
    shift: u32,
    context: TaskContext,
) -> Result<DecimalU64> {
    let controller = context.lease_resource(&controller)?;
    {
        let mut state = controller.state.lock().unwrap();
        state.version = state
            .version
            .checked_add(1)
            .ok_or_else(|| Error::new("fixture_version", "View version exhausted"))?;
        state.shift = shift;
    }
    controller
        .jobs
        .capture(&context, "scene", |state| Ok(state.shift))?;
    Ok(controller
        .state
        .lock()
        .unwrap()
        .scene
        .snapshot()
        .revision
        .into())
}
