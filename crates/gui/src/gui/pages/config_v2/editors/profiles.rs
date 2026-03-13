use super::*;

mod local;
mod remote;

pub(in super::super) use local::{LocalProfilesSectionArgs, render_config_v2_profiles_local};
pub(in super::super) use remote::render_config_v2_profiles_control_plane;
