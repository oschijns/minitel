//! Wi-Fi connector backed by real ESP32 hardware, driven through `esp-idf-svc`.

use crate::app::WifiConnector;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    sys::EspError,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};

pub struct EspWifiConnector {
    wifi: BlockingWifi<EspWifi<'static>>,
}

impl EspWifiConnector {
    pub fn new(
        modem: Modem<'static>,
        sys_loop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
    ) -> Result<Self, EspError> {
        let esp_wifi = EspWifi::new(modem, sys_loop.clone(), Some(nvs))?;
        let wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;
        Ok(Self { wifi })
    }
}

impl WifiConnector for EspWifiConnector {
    async fn connect(&mut self, ssid: &str, password: &str) -> Result<String, String> {
        // Stop first in case a previous attempt left the driver half-configured; ignore the
        // error since it's expected to fail if it was never started.
        let _ = self.wifi.stop();

        let mut client_config = ClientConfiguration {
            auth_method: if password.is_empty() {
                AuthMethod::None
            } else {
                AuthMethod::WPA2Personal
            },
            ..Default::default()
        };
        client_config.ssid = ssid
            .try_into()
            .map_err(|_| "SSID trop long (32 caracteres max)".to_string())?;
        client_config.password = password
            .try_into()
            .map_err(|_| "Mot de passe trop long (64 caracteres max)".to_string())?;

        self.wifi
            .set_configuration(&Configuration::Client(client_config))
            .map_err(|e| e.to_string())?;
        self.wifi.start().map_err(|e| e.to_string())?;
        self.wifi.connect().map_err(|e| e.to_string())?;
        self.wifi.wait_netif_up().map_err(|e| e.to_string())?;

        let ip_info = self
            .wifi
            .wifi()
            .sta_netif()
            .get_ip_info()
            .map_err(|e| e.to_string())?;

        Ok(ip_info.ip.to_string())
    }
}
