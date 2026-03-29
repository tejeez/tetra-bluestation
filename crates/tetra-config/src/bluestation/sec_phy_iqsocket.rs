use serde::Deserialize;
use std::collections::HashMap;
use toml::Value;

#[derive(Debug, Clone)]
pub struct CfgIqSocket {
    pub rx_path: String,
    pub tx_path: String,
}

#[derive(Deserialize, Default)]
pub struct IqSocketDto {
    pub rx_path: Option<String>,
    pub tx_path: Option<String>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl IqSocketDto {
    pub fn to_cfg(self) -> CfgIqSocket {
        CfgIqSocket {
            rx_path: self.rx_path.unwrap_or("/tmp/bluestation-rx-socket".to_string()),
            tx_path: self.tx_path.unwrap_or("/tmp/bluestation-tx-socket".to_string()),
        }
    }
}
