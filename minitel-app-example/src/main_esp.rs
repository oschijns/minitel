use log::{error, info, warn};
use minitel::{
    prelude::*,
    stum::protocol::{Baudrate, RoutingRx, RoutingTx},
};
use std::thread::sleep;

use crate::app::App;

// embassy-executor is pulled in transitively by embassy-time-queue-utils and
// requires a __pender symbol. We don't use the Embassy executor (we use tokio),
// so provide a no-op stub.
#[export_name = "__pender"]
fn __pender(_context: *mut ()) {}

pub fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let _mounted_eventfs = esp_idf_svc::io::vfs::MountedEventfs::mount(1).expect("Failed to initialize eventfd");

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
    // Initialize the minitel
    let mut minitel = minitel::esp::esp_minitel_uart2().unwrap();
    minitel.search_speed().await.unwrap();
    // TODO: re-enable once rendering is confirmed working at 1200 baud
    // if let Err(e) = minitel.set_speed(Baudrate::B9600).await {
    //     warn!("Failed to switch to 9600 baud ({e}), staying at current speed");
    // }

    // Drain any stale data left in the UART buffer from the baud rate transition
    while minitel.read_byte_blocking().is_ok() {}

    if let Err(e) = minitel
        .set_routing(false, RoutingRx::Modem, RoutingTx::Keyboard)
        .await
    {
        warn!("Failed to set routing ({e}), continuing anyway");
    }

    // Run the app
    App::default().run(&mut minitel).await.unwrap();

    if let Err(e) = minitel
        .set_routing(true, RoutingRx::Modem, RoutingTx::Keyboard)
        .await
    {
        warn!("Failed to restore routing ({e})");
    }
    Ok(())
}
