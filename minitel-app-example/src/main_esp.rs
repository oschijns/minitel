use crate::app::App;
use crate::wifi_esp::EspWifiConnector;
use log::{error, info};
use minitel::{
    prelude::*,
    stum::protocol::{Baudrate, RoutingRx, RoutingTx},
};
use std::thread::sleep;

pub fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    // Kept alive for the whole program: dropping it would unregister the eventfd
    // pseudo-filesystem that Tokio's reactor relies on.
    let _eventfd =
        esp_idf_svc::io::vfs::MountedEventfs::mount(1).expect("Failed to initialize eventfd");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build Tokio runtime");

    match rt.block_on(async { async_main().await }) {
        Ok(()) => info!("main() finished, reboot."),
        Err(err) => {
            error!("{err:?}");
            // Let them read the error message before rebooting
            sleep(std::time::Duration::from_secs(3));
        }
    }

    esp_idf_svc::hal::reset::restart();
}

async fn async_main() -> std::io::Result<()> {
    // Initialize the minitel. The modem peripheral is left over for the Wi-Fi connector below.
    let (mut minitel, modem) = minitel::esp::esp_minitel_uart2().unwrap();
    minitel.search_speed().await.unwrap();
    // Minitel version 1 only support a max baudrate of 1200
    minitel.set_speed(Baudrate::B1200).await.unwrap();
    minitel
        .set_routing(false, RoutingRx::Modem, RoutingTx::Keyboard)
        .await
        .unwrap();

    let sys_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take().unwrap();
    let nvs = esp_idf_svc::nvs::EspDefaultNvsPartition::take().unwrap();
    let wifi_connector = EspWifiConnector::new(modem, sys_loop, nvs).unwrap();

    // Run the app
    App::with_wifi_connector(wifi_connector)
        .run(&mut minitel)
        .await
        .unwrap();

    minitel
        .set_routing(true, RoutingRx::Modem, RoutingTx::Keyboard)
        .await
        .unwrap();
    Ok(())
}
