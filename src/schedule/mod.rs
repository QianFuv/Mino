//! Scheduler-neutral, side-effect-free task handoff specifications.

mod spec;

pub use spec::{
    SCHEDULE_SPEC_KIND, ScheduleSpecRequest, ScheduleSpecService, ScheduledAuthorization,
    ScheduledExecution, ScheduledMonitorPolicy, ScheduledOutcomePolicy, ScheduledProjectContext,
    ScheduledSideEffects, ScheduledTaskSpec, ScheduledTrigger,
};
