//! The compute layer (Stage 3): turns closed sweeps into GPU-ready grids.
//! Sits between assembly and the applier — `poller → assembly → compute →
//! applier → AppState` — and produces exactly the bytes a texture will hold
//! plus the metadata a shader will need as uniforms. It draws nothing: no
//! wgpu, no window, no shader. That is Stage 4.
//!
//! [`DisplayProduct`] is distinct from `nexrad_decoder::ProductKind`: it
//! also covers products the decoder never sees (Echo Tops, VIL) and
//! excludes the moments Q8 deferred (PHI, CFP). [`grid::SweepGrid`] is a
//! single-channel 8-bit grid plus scale/offset (ADR-0020's R8 + LUT
//! representation) — see `grid.rs` for the gridding algorithm and
//! `palette.rs` for how a grid's cell values become colour.

pub mod derived;
pub mod geometry;
pub mod grid;
pub mod palette;

#[cfg(test)]
mod test_support;

use std::sync::Arc;

use nexrad_decoder::{ProductKind, VolumeStatus};
use tokio::sync::{mpsc, watch};

use crate::assembly::{AssemblyEvent, VolumeId};
use crate::event::Event;
use crate::state::{AppState, VolumeSummary};

pub use grid::SweepGrid;

/// A product the user can select for display. Distinct from
/// `nexrad_decoder::ProductKind` because it also covers products the
/// decoder never sees (Echo Tops, VIL) and excludes the moments Q8
/// deferred (PHI, CFP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DisplayProduct {
    Reflectivity,
    Velocity,
    SpectrumWidth,
    Zdr,
    Cc,
    EchoTops,
    Vil,
}

impl DisplayProduct {
    /// The five products gridded directly from a decoded sweep, paired with
    /// the decoder moment each comes from. Echo Tops and VIL are absent —
    /// they are volume-derived and have no source moment. Declaration order
    /// doubles as display order: `grid_all_base_products` grids in this
    /// order, which is also `DisplayProduct`'s `Ord` order, so a
    /// `Vec<Arc<SweepGrid>>` built by iterating it is sorted for free.
    pub const BASE: [(DisplayProduct, ProductKind); 5] = [
        (DisplayProduct::Reflectivity, ProductKind::Ref),
        (DisplayProduct::Velocity, ProductKind::Vel),
        (DisplayProduct::SpectrumWidth, ProductKind::SpectrumWidth),
        (DisplayProduct::Zdr, ProductKind::Zdr),
        (DisplayProduct::Cc, ProductKind::Rho),
    ];

    /// Every product this stage knows about, base and derived, in display
    /// order. Backs `palette::load_all`'s iteration and anywhere else that
    /// needs the full set rather than just the sweep-gridded ones.
    pub const ALL: [DisplayProduct; 7] = [
        DisplayProduct::Reflectivity,
        DisplayProduct::Velocity,
        DisplayProduct::SpectrumWidth,
        DisplayProduct::Zdr,
        DisplayProduct::Cc,
        DisplayProduct::EchoTops,
        DisplayProduct::Vil,
    ];

    /// The decoder moment this product is gridded from, or `None` for a
    /// volume-derived product (Echo Tops, VIL) that has no source moment.
    pub fn source_moment(self) -> Option<ProductKind> {
        Self::BASE.iter().find(|(p, _)| *p == self).map(|(_, k)| *k)
    }
}

impl std::fmt::Display for DisplayProduct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Reflectivity => "ref",
            Self::Velocity => "vel",
            Self::SpectrumWidth => "sw",
            Self::Zdr => "zdr",
            Self::Cc => "cc",
            Self::EchoTops => "echo_tops",
            Self::Vil => "vil",
        })
    }
}

/// What the compute layer hands the applier. Distinct from `AssemblyEvent`
/// because everything below this point deals in grids, not radials.
pub enum StateUpdate {
    SweepGridded {
        elevation_number: u8,
        elevation_deg: f32,
        volume: VolumeId,
        vcp_number: u16,
        /// One entry per base product actually present on this sweep,
        /// sorted by `DisplayProduct` (see `BASE`'s doc comment).
        grids: Vec<Arc<SweepGrid>>,
    },
    DerivedComputed {
        volume: VolumeId,
        vcp_number: u16,
        /// EchoTops, Vil — whichever could be derived.
        grids: Vec<Arc<SweepGrid>>,
    },
    VolumeClosed {
        summary: VolumeSummary,
    },
    /// Pass-through for the assembler's observability events, which the
    /// compute layer neither consumes nor interprets.
    Info(AssemblyEvent),
}

/// No VCP flown by WSR-88D approaches this many elevation cuts; a retained
/// set larger than this means a volume is stuck open and must not be
/// allowed to grow without bound (S3-W2 §5.4).
const RETAINED_ELEVATION_CAP: usize = 40;

/// Reflectivity grids retained across an accumulating volume, one per
/// closed elevation, in closure order — needed to derive Echo Tops/VIL at
/// volume close (§7.2) and free to hold because `AppState` already owns the
/// same allocations (`Arc` clones only). Bounded per §5.4: a volume that
/// never closes must not turn this into a slow leak.
struct RetainedTilts {
    tilts: Vec<Arc<SweepGrid>>,
}

impl RetainedTilts {
    fn new() -> Self {
        Self { tilts: Vec::new() }
    }

    /// Keep `grid` if it is the sweep's reflectivity product. Returns an
    /// event if the cap was hit and the oldest retained tilt was dropped.
    fn offer(&mut self, grid: &Arc<SweepGrid>) -> Option<Event> {
        if grid.product != DisplayProduct::Reflectivity {
            return None;
        }
        self.tilts.push(Arc::clone(grid));
        if self.tilts.len() > RETAINED_ELEVATION_CAP {
            let dropped = self.tilts.remove(0);
            return Some(Event::RetainedGridSetBounded { dropped_elevation_number: dropped.elevation_number });
        }
        None
    }

    fn take(&mut self) -> Vec<Arc<SweepGrid>> {
        std::mem::take(&mut self.tilts)
    }

    fn clear(&mut self) {
        self.tilts.clear();
    }
}

/// Drives the compute stage from a channel of assembly events, gridding
/// each closed sweep and deriving Echo Tops/VIL when a volume completes.
/// Returns when `rx` closes, `tx`'s receiver is dropped, or `shutdown`
/// publishes `true` — same shape as `assembly::run`/`pipeline::apply_loop`.
///
/// Gridding runs on `spawn_blocking`, awaited inline: the runtime has two
/// workers (S2-c), and a multi-millisecond synchronous burst on one of them
/// would delay the poller's next fetch if it ran inline on this task's
/// executor thread instead. Awaiting inline (rather than spawning and
/// moving on) means at most one grid job runs at a time — bounded, and it
/// is what keeps the "no rayon yet" decision (§3.6) honest. A panic inside
/// the blocking closure propagates out of this function: the
/// poller/assembly/compute/applier quartet is supervised as one unit
/// (`pipeline.rs`), so that is the same recovery path a panic anywhere else
/// in the quartet already takes, not a new failure mode.
pub async fn compute_loop(
    mut rx: mpsc::Receiver<AssemblyEvent>,
    tx: mpsc::Sender<StateUpdate>,
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut retained = RetainedTilts::new();

    loop {
        // The received event is extracted inside `select!` but handled
        // outside it: a branch body that itself `.await`s (as handling a
        // sweep does, via `spawn_blocking` and the channel send) makes
        // `select!`'s generated future require every branch's output to be
        // `Send`, including `shutdown.wait_for`'s `watch::Ref` — which
        // isn't. `assembly::run` and `pipeline::apply_loop` avoid this the
        // same way: `select!` only ever extracts a value; every `.await`
        // happens after it.
        let event = tokio::select! {
            event = rx.recv() => event,
            _ = shutdown.wait_for(|s| *s) => return,
        };
        let Some(event) = event else { return };
        if !handle_event(event, &tx, &state, &mut retained).await {
            return;
        }
    }
}

/// Returns `false` when the applier's receiver has gone away and the loop
/// should stop.
async fn handle_event(
    event: AssemblyEvent,
    tx: &mpsc::Sender<StateUpdate>,
    state: &Arc<AppState>,
    retained: &mut RetainedTilts,
) -> bool {
    match event {
        AssemblyEvent::SweepClosed { sweep, volume, vcp_number } => {
            let elevation_number = sweep.elevation_number;
            let elevation_deg = sweep.elevation_deg;
            let (grids, events) = tokio::task::spawn_blocking(move || grid::grid_all_base_products(&sweep))
                .await
                .expect("grid_all_base_products panicked");
            for event in events {
                state.report(event);
            }
            for grid in &grids {
                if let Some(event) = retained.offer(grid) {
                    state.report(event);
                }
            }
            tx.send(StateUpdate::SweepGridded { elevation_number, elevation_deg, volume, vcp_number, grids })
                .await
                .is_ok()
        }
        AssemblyEvent::VolumeClosed { volume } => {
            let summary = VolumeSummary::from_scan(&volume);
            if volume.status == VolumeStatus::Complete {
                let tilts = retained.take();
                let vol_id = summary.volume;
                let vcp_number = summary.vcp_number;
                let (derived_grids, events) = tokio::task::spawn_blocking(move || derived::compute_derived(&tilts))
                    .await
                    .expect("compute_derived panicked");
                for event in events {
                    state.report(event);
                }
                if !derived_grids.is_empty()
                    && tx
                        .send(StateUpdate::DerivedComputed { volume: vol_id, vcp_number, grids: derived_grids })
                        .await
                        .is_err()
                {
                    return false;
                }
            }
            retained.clear();
            tx.send(StateUpdate::VolumeClosed { summary }).await.is_ok()
        }
        other => tx.send(StateUpdate::Info(other)).await.is_ok(),
    }
}
