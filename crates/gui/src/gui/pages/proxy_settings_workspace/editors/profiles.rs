use super::*;

mod local;
mod remote;
mod shared;

pub(in super::super) use local::{LocalProfilesSectionArgs, render_proxy_settings_profiles_local};
pub(in super::super) use remote::render_proxy_settings_profiles_control_plane;
