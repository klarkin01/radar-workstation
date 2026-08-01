//! Volume assembly state machine (ADR-0012). Turns the flat `Vec<Radial>`
//! stream the decoder produces per chunk into `VolumeScan`s, closing sweeps
//! incrementally so partial-scan rendering is possible before the volume
//! closes.
//!
//! [`VolumeAssembler`] is a pure, synchronous core: no `async`, no I/O, no
//! clock access. `now` is a parameter on every call rather than read
//! internally, which is what makes the watchdog timeout — the one
//! ADR-0012 exit path that would otherwise need a real ten-minute wait to
//! test — an ordinary unit test. [`run`] is the async wrapper that reads
//! chunks off a channel and drives the watchdog on a timer; it stays a thin
//! shell around the core on purpose — if it grows logic, that logic belongs
//! above instead.

mod context;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexrad_decoder::{Radial, RadialStatus, Sweep, VcpDefinition, VolumeScan, VolumeStatus};

use crate::chunk::ChunkKind;
use context::VolumeContext;

/// ADR-0012: "approximately 10-15 minutes (well beyond the longest VCP
/// cycle)". 12 minutes splits the difference. Not user-configurable at this
/// stage; FR-CP-1 may expose it later if operational experience shows it's
/// worth it.
const WATCHDOG_TIMEOUT_DEFAULT: Duration = Duration::from_secs(12 * 60);

pub struct AssemblyConfig {
    pub watchdog_timeout: Duration,
}

/// Identifies which volume a sweep or scan came from. Ordered by
/// `(julian_date, scan_time_ms)` — `scan_time_ms` is milliseconds since
/// midnight UTC and wraps daily, so it is never compared on its own. Lives
/// here (not `state`) because its fields are exactly the identifying subset
/// of `VolumeContext`/`VolumeScan`, computed at sweep-closure time from data
/// `VolumeAssembler` already holds — `state::apply` (S2-W1) consumes it but
/// does not derive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VolumeId {
    pub julian_date: u16,
    pub scan_time_ms: u32,
}

impl Default for AssemblyConfig {
    fn default() -> Self {
        Self { watchdog_timeout: WATCHDOG_TIMEOUT_DEFAULT }
    }
}

/// Mirrors ADR-0012's state diagram exactly. The ADR's "tentatively closed
/// sweep" state was explicitly considered and rejected (§Considered
/// Alternatives) — do not reintroduce it here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssemblyState {
    Idle,
    AwaitingData,
    Accumulating,
}

#[derive(Debug)]
pub enum AssemblyEvent {
    /// `volume` and `vcp_number` are the accumulating volume's identity and
    /// VCP at the moment this sweep closed — read from `VolumeContext`,
    /// which is always populated by the time any sweep can close (the first
    /// radial of the volume sets it before any elevation-closure signal is
    /// possible).
    SweepClosed { sweep: Arc<Sweep>, volume: VolumeId, vcp_number: u16 },
    VolumeClosed { volume: VolumeScan },
    /// Radials arrived for an elevation number whose sweep is already
    /// closed. `count` radials were discarded, unmutated, for that one
    /// `on_chunk` call — sweep closure is permanent per ADR-0012.
    LateRadialsDiscarded { elevation_number: u8, count: usize },
    /// A `-S` chunk was never seen for the volume now accumulating.
    MissingStartChunk,
}

/// Accumulates radials for the sweep currently open.
struct SweepBuilder {
    elevation_number: u8,
    elevation_deg: f32,
    nyquist_velocity_mps: Option<f32>,
    unambiguous_range_km: Option<f32>,
    radials: Vec<Radial>,
}

impl SweepBuilder {
    fn start(radial: Radial) -> Self {
        Self {
            elevation_number: radial.elevation_number,
            elevation_deg: radial.elevation_deg,
            nyquist_velocity_mps: radial.nyquist_velocity_mps,
            unambiguous_range_km: radial.unambiguous_range_km,
            radials: vec![radial],
        }
    }

    fn push(&mut self, radial: Radial) {
        self.radials.push(radial);
    }

    fn close(self, complete: bool) -> Sweep {
        Sweep {
            elevation_number: self.elevation_number,
            elevation_deg: self.elevation_deg,
            nyquist_velocity_mps: self.nyquist_velocity_mps,
            unambiguous_range_km: self.unambiguous_range_km,
            radials: self.radials,
            complete,
        }
    }
}

pub struct VolumeAssembler {
    config: AssemblyConfig,
    state: AssemblyState,
    context: VolumeContext,
    current_sweep: Option<SweepBuilder>,
    closed_sweeps: Vec<Arc<Sweep>>,
    closed_elevations: HashSet<u8>,
    last_activity: Option<Instant>,
}

impl VolumeAssembler {
    pub fn new(config: AssemblyConfig) -> Self {
        Self {
            config,
            state: AssemblyState::Idle,
            context: VolumeContext::default(),
            current_sweep: None,
            closed_sweeps: Vec::new(),
            closed_elevations: HashSet::new(),
            last_activity: None,
        }
    }

    pub fn state(&self) -> AssemblyState {
        self.state
    }

    /// Returns to `Idle`, dropping all in-flight state. FR-DA-4 (site
    /// change) consumes this in a later stage.
    pub fn reset(&mut self) {
        self.state = AssemblyState::Idle;
        self.context = VolumeContext::default();
        self.current_sweep = None;
        self.closed_sweeps = Vec::new();
        self.closed_elevations = HashSet::new();
        self.last_activity = None;
    }

    /// Feed one decoded `-S` chunk. `vcp` is the decoded VCP definition
    /// from Message 5, if the `-S` chunk's metadata stream contained one
    /// (S1-W2) — `None` falls back to ADR-0012's missing-`-S` handling,
    /// which is exactly what happens if this is never called for a volume
    /// at all (see `on_chunk`'s own defensive `ChunkKind::Start` handling
    /// for that case).
    pub fn on_start_chunk(&mut self, vcp: Option<VcpDefinition>, now: Instant) -> Vec<AssemblyEvent> {
        let mut events = Vec::new();
        self.last_activity = Some(now);
        self.begin_new_volume(&mut events);
        if let Some(vcp) = vcp {
            self.context.apply_vcp(vcp);
        }
        events
    }

    /// Feed one decompressed, decoded chunk. `radials` is empty for `-S`
    /// chunks (they carry no Message 31 data in this codebase's decoder) —
    /// a `-S` chunk should normally go through [`Self::on_start_chunk`]
    /// instead, which also decodes its VCP definition; `on_chunk` still
    /// handles `ChunkKind::Start` defensively (with no VCP) so it remains
    /// total regardless of which entry point a caller uses.
    pub fn on_chunk(&mut self, kind: ChunkKind, radials: Vec<Radial>, now: Instant) -> Vec<AssemblyEvent> {
        let mut events = Vec::new();
        self.last_activity = Some(now);

        if kind == ChunkKind::Start {
            self.begin_new_volume(&mut events);
            return events;
        }

        if self.state == AssemblyState::Idle {
            // Missing -S entirely: an -I or -E chunk arrives with nothing
            // in progress. Emit the signal and proceed anyway — the
            // pipeline never blocks waiting for a chunk that may never
            // arrive.
            events.push(AssemblyEvent::MissingStartChunk);
            self.state = AssemblyState::AwaitingData;
        }

        let mut late_discards: HashMap<u8, usize> = HashMap::new();

        for radial in radials {
            if self.closed_elevations.contains(&radial.elevation_number) {
                *late_discards.entry(radial.elevation_number).or_insert(0) += 1;
                continue;
            }

            let is_first_radial_of_volume = self.state == AssemblyState::AwaitingData;
            if is_first_radial_of_volume {
                self.state = AssemblyState::Accumulating;
            }
            self.context.apply_radial(&radial, is_first_radial_of_volume);

            self.ingest_radial(radial, &mut events);
        }

        for (elevation_number, count) in late_discards {
            events.push(AssemblyEvent::LateRadialsDiscarded { elevation_number, count });
        }

        if kind == ChunkKind::End {
            // Normally already closed by this chunk's EndOfVolume radial;
            // this is a defensive fallback for a malformed -E chunk that
            // never carries one.
            if let Some(builder) = self.current_sweep.take() {
                self.close_sweep(builder, false, &mut events);
            }
            self.finish_volume(VolumeStatus::Complete, &mut events);
        }

        events
    }

    /// Drive the watchdog. Called on a timer by the owning task.
    pub fn on_tick(&mut self, now: Instant) -> Vec<AssemblyEvent> {
        let mut events = Vec::new();

        if self.state == AssemblyState::Idle {
            return events;
        }
        let Some(last) = self.last_activity else {
            return events;
        };
        if now.duration_since(last) >= self.config.watchdog_timeout {
            if let Some(builder) = self.current_sweep.take() {
                self.close_sweep(builder, false, &mut events);
            }
            self.finish_volume(VolumeStatus::TimedOut, &mut events);
        }

        events
    }

    /// Shared by `on_chunk(ChunkKind::Start, ...)` and `on_start_chunk`:
    /// supersede whatever volume was accumulating (if any) and reset for a
    /// new one. Does not touch VCP metadata — callers apply that after.
    fn begin_new_volume(&mut self, events: &mut Vec<AssemblyEvent>) {
        if self.state == AssemblyState::Accumulating {
            // Early -S before -E: the new volume begins immediately in
            // this same call.
            if let Some(builder) = self.current_sweep.take() {
                self.close_sweep(builder, false, events);
            }
            self.finish_volume(VolumeStatus::Superseded, events);
        }
        self.context = VolumeContext::default();
        self.current_sweep = None;
        self.closed_sweeps = Vec::new();
        self.closed_elevations = HashSet::new();
        self.state = AssemblyState::AwaitingData;
    }

    /// Route one radial into the sweep it belongs to, closing the
    /// previously-open sweep first if the elevation number changed.
    fn ingest_radial(&mut self, radial: Radial, events: &mut Vec<AssemblyEvent>) {
        let is_closure_status =
            matches!(radial.radial_status, RadialStatus::EndOfElevation | RadialStatus::EndOfVolume);
        let elevation_number = radial.elevation_number;

        match self.current_sweep.take() {
            None => {
                self.current_sweep = Some(SweepBuilder::start(radial));
            }
            Some(mut builder) if builder.elevation_number == elevation_number => {
                builder.push(radial);
                self.current_sweep = Some(builder);
            }
            Some(builder) => {
                // New elevation number arrived without an EndOfElevation
                // on the previous one: close it as incomplete.
                self.close_sweep(builder, false, events);
                self.current_sweep = Some(SweepBuilder::start(radial));
            }
        }

        if is_closure_status {
            if let Some(builder) = self.current_sweep.take() {
                self.close_sweep(builder, true, events);
            }
        }
    }

    fn close_sweep(&mut self, builder: SweepBuilder, complete: bool, events: &mut Vec<AssemblyEvent>) {
        let elevation_number = builder.elevation_number;
        let sweep = Arc::new(builder.close(complete));
        self.closed_elevations.insert(elevation_number);
        self.closed_sweeps.push(Arc::clone(&sweep));
        let volume = VolumeId { julian_date: self.context.julian_date, scan_time_ms: self.context.scan_time_ms };
        events.push(AssemblyEvent::SweepClosed { sweep, volume, vcp_number: self.context.vcp_number() });
    }

    fn finish_volume(&mut self, status: VolumeStatus, events: &mut Vec<AssemblyEvent>) {
        let volume = VolumeScan {
            site_id: self.context.site_id,
            scan_time_ms: self.context.scan_time_ms,
            julian_date: self.context.julian_date,
            vcp_number: self.context.vcp_number(),
            latitude: self.context.latitude,
            longitude: self.context.longitude,
            site_amsl_m: self.context.site_amsl_m,
            sweeps: std::mem::take(&mut self.closed_sweeps),
            status,
        };
        events.push(AssemblyEvent::VolumeClosed { volume });
        self.state = AssemblyState::Idle;
        self.context = VolumeContext::default();
        self.closed_elevations = HashSet::new();
    }
}

/// How often [`run`] drives [`VolumeAssembler::on_tick`] independently of
/// chunk arrivals. Well under the watchdog timeout so a stalled volume is
/// noticed promptly rather than only on the next chunk (which may never
/// come — that is exactly the case the watchdog exists for).
const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Decode and feed one raw chunk to `assembler`, using `now` for both the
/// decode-failure path and the successful `on_chunk` call.
///
/// A decode failure here is silently dropped rather than surfaced — FR-ND-7
/// requires that a decode failure not crash or freeze the application, and
/// dropping one chunk (a bounded gap, exactly like a chunk that never
/// arrived at all) satisfies that. Making the failure *observable* is
/// S1-W3c's typed event/logging work, not duplicated here ahead of it.
fn decode_and_ingest(
    assembler: &mut VolumeAssembler,
    envelope: crate::ingest::ChunkEnvelope,
    now: Instant,
) -> Vec<AssemblyEvent> {
    let Ok(decompressed) = crate::chunk::decompress_chunk(&envelope.raw_bytes, envelope.kind) else {
        return Vec::new();
    };

    if envelope.kind == ChunkKind::Start {
        // A missing or unreadable Message 5 is not a hard failure — the
        // RVOL fallback (VolumeContext::apply_radial) covers it.
        let vcp = nexrad_decoder::parse_metadata_stream(&decompressed).ok().and_then(|m| m.vcp);
        return assembler.on_start_chunk(vcp, now);
    }

    let Ok(radials) = nexrad_decoder::parse_radial_stream(&decompressed) else {
        return Vec::new();
    };
    assembler.on_chunk(envelope.kind, radials, now)
}

/// Drives a [`VolumeAssembler`] from a channel of raw chunks, decoding each
/// one and forwarding assembly events to `tx`. Returns when `rx` closes,
/// `tx`'s receiver is dropped, or `shutdown` publishes `true` (S2-W2 §4.2) —
/// checked promptly via `select!` rather than only at the next chunk or tick,
/// which may not come for a while (that's exactly the case the watchdog
/// exists for).
pub async fn run(
    mut rx: tokio::sync::mpsc::Receiver<crate::ingest::ChunkEnvelope>,
    tx: tokio::sync::mpsc::Sender<AssemblyEvent>,
    config: AssemblyConfig,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut assembler = VolumeAssembler::new(config);
    let mut ticker = tokio::time::interval(TICK_INTERVAL);

    loop {
        let events = tokio::select! {
            chunk = rx.recv() => {
                let Some(envelope) = chunk else { return };
                decode_and_ingest(&mut assembler, envelope, Instant::now())
            }
            _ = ticker.tick() => assembler.on_tick(Instant::now()),
            _ = shutdown.wait_for(|s| *s) => return,
        };
        for event in events {
            if tx.send(event).await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
