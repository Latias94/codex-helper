pub(super) use super::stations_detail_persisted_config::*;
pub(super) use super::stations_detail_quick_switch::*;
pub(super) use super::stations_detail_runtime_control::*;

use super::*;

pub(super) fn refresh_runtime_snapshot(ctx: &mut PageCtx<'_>) {
    ctx.proxy
        .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
}
