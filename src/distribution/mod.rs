//! Canonical plugin source and native distribution contracts.

mod contract;

pub use contract::{
    MINO_PLUGIN_CONTRACT_KIND, PluginContractReport, validate_mino_plugin_source,
    validate_plugin_source,
};
