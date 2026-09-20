use crate::app::App;
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

    match futures::executor::block_on(async_main()) {
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
    // Initialize the minitel
    let mut minitel = minitel::esp::esp_minitel_uart2().unwrap();
    minitel.search_speed().await.unwrap();

    // Minitel version 1 only support a max baudrate of 1200
    // whereas Minitel version 2 can go up to 9600

    #[cfg(not(feature = "minitel2"))]
    const BAUD: Baudrate = Baudrate::B1200;

    #[cfg(feature = "minitel2")]
    const BAUD: Baudrate = Baudrate::B9600;

    minitel.set_speed(BAUD).await.unwrap();

    minitel
        .set_routing(false, RoutingRx::Modem, RoutingTx::Keyboard)
        .await
        .unwrap();

    // Run the app
    App::default().run(&mut minitel).await.unwrap();

    minitel
        .set_routing(true, RoutingRx::Modem, RoutingTx::Keyboard)
        .await
        .unwrap();
    Ok(())
}
