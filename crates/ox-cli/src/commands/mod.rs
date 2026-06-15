//! CLI command implementations.
//!
//! Each subcommand has its own module with argument definitions and execution logic.
//! Stub commands are grouped together in the `stubs` module.

mod cancel;
mod check_consistency;
mod clean;
mod common;
mod dag;
mod dashboard;
mod explain;
mod export;
mod gate;
mod guide;
mod history;
mod init;
mod invalidate;
mod lint;
mod lock;
mod logo;
mod logs;
mod plan;
mod query;
mod run;
mod serve;
mod snapshot;
mod status;
mod stubs;
mod subscribe;
mod test;
mod translate;

pub use cancel::{CancelArgs, cmd_cancel};
pub use check_consistency::{CheckConsistencyArgs, cmd_check_consistency};
pub use clean::{CleanArgs, cmd_clean};
pub use dag::{DagArgs, cmd_dag};
pub use dashboard::{DashboardArgs, cmd_dashboard};
pub use explain::{ExplainArgs, cmd_explain};
pub use export::{ExportArgs, cmd_export};
pub use gate::{GateArgs, cmd_gate};
pub use guide::{HANDBOOK, cmd_guide};
pub use history::{HistoryArgs, cmd_history};
pub use init::{InitArgs, cmd_init};
pub use invalidate::{InvalidateArgs, cmd_invalidate};
pub use lint::{LintArgs, cmd_lint};
pub use lock::{LockArgs, cmd_lock};
pub use logo::cmd_logo;
pub use logs::{LogsArgs, cmd_logs};
pub use plan::{PlanArgs, cmd_plan};
pub use query::{QueryArgs, cmd_query};
pub use run::{RunArgs, cmd_run};
pub use serve::{ServeArgs, cmd_serve};
pub use snapshot::{SnapshotArgs, cmd_snapshot};
pub use status::{StatusArgs, cmd_status};
pub use stubs::{TopArgs, cmd_top};
pub use subscribe::{SubscribeArgs, cmd_subscribe};
pub use test::{TestArgs, cmd_test};
pub use translate::{TranslateArgs, cmd_translate};
