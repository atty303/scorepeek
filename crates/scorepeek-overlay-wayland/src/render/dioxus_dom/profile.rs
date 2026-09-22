use super::*;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(super) struct WorkStat {
    pub(super) calls: u64,
    pub(super) total_ns: u64,
}

#[derive(Clone, Default, Serialize)]
pub(crate) struct FrameWorkProfile {
    phases: std::collections::BTreeMap<&'static str, WorkStat>,
    unmeasured_calls: std::collections::BTreeMap<&'static str, u64>,
    pub(super) frames: std::collections::VecDeque<FrameWorkSample>,
    dropped_frames: u64,
}

#[derive(Clone, Default, Serialize)]
pub(super) struct FrameWorkSample {
    pub(super) sequence: u64,
    pub(super) phases: std::collections::BTreeMap<&'static str, WorkStat>,
    pub(super) unmeasured_calls: std::collections::BTreeMap<&'static str, u64>,
    pub(super) live_canvases: u64,
    pub(super) live_widgets: u64,
}

impl FrameWorkProfile {
    const FRAME_CAPACITY: usize = 256;
    pub(super) const REQUIRED_PHASES: [&'static str; 16] = [
        "dioxus_poll",
        "projection_rebuild",
        "canvas_config",
        "package_open",
        "package_clone",
        "wasm_runtime_create",
        "skin_input",
        "wasm_render",
        "json_tree",
        "tree_reconciliation",
        "resource_lookup",
        "resource_decode",
        "blitz_layout",
        "scene",
        "gpu_present",
        "surface_commit",
    ];

    pub(crate) fn record(&mut self, phase: &'static str, elapsed: Duration) {
        self.record_stat(
            phase,
            WorkStat {
                calls: 1,
                total_ns: u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
            },
        );
    }

    pub(super) fn record_stat(&mut self, phase: &'static str, delta: WorkStat) {
        let stat = self.phases.entry(phase).or_default();
        stat.calls = stat.calls.saturating_add(delta.calls);
        stat.total_ns = stat.total_ns.saturating_add(delta.total_ns);
    }

    pub(super) fn measure<T>(&mut self, phase: &'static str, operation: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = operation();
        self.record(phase, started.elapsed());
        result
    }

    pub(crate) fn unmeasured(&mut self, phase: &'static str) {
        let calls = self.unmeasured_calls.entry(phase).or_default();
        *calls = calls.saturating_add(1);
    }

    pub(super) fn snapshot(&self) -> FrameWorkSample {
        FrameWorkSample {
            sequence: self
                .dropped_frames
                .saturating_add(u64::try_from(self.frames.len()).unwrap_or(u64::MAX))
                .saturating_add(1),
            phases: self.phases.clone(),
            unmeasured_calls: self.unmeasured_calls.clone(),
            live_canvases: 0,
            live_widgets: 0,
        }
    }

    pub(super) fn finish_frame(
        &mut self,
        before: &FrameWorkSample,
        live_canvases: u64,
        live_widgets: u64,
    ) {
        let mut phases = self
            .phases
            .iter()
            .filter_map(|(phase, after)| {
                let before = before.phases.get(phase).copied().unwrap_or_default();
                let delta = WorkStat {
                    calls: after.calls.saturating_sub(before.calls),
                    total_ns: after.total_ns.saturating_sub(before.total_ns),
                };
                (delta.calls > 0).then_some((*phase, delta))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        for phase in Self::REQUIRED_PHASES {
            phases.entry(phase).or_default();
        }
        let unmeasured_calls = self
            .unmeasured_calls
            .iter()
            .filter_map(|(phase, after)| {
                let delta = after.saturating_sub(*before.unmeasured_calls.get(phase).unwrap_or(&0));
                (delta > 0).then_some((*phase, delta))
            })
            .collect();
        if self.frames.len() == Self::FRAME_CAPACITY {
            self.frames.pop_front();
            self.dropped_frames = self.dropped_frames.saturating_add(1);
        }
        self.frames.push_back(FrameWorkSample {
            sequence: before.sequence,
            phases,
            unmeasured_calls,
            live_canvases,
            live_widgets,
        });
    }

    #[cfg(test)]
    pub(super) fn calls(&self, phase: &'static str) -> u64 {
        self.phases.get(phase).map_or(0, |stat| stat.calls)
    }
}
