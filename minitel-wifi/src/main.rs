mod app;

use crate::app::App;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{gpio::AnyIOPin, peripherals::Peripherals, uart},
    nvs::EspDefaultNvsPartition,
    wifi::{ClientConfiguration, Configuration, EspWifi},
};
use log::{error, info};
use minitel::{
    prelude::*,
    stum::protocol::{Baudrate, RoutingRx, RoutingTx},
};
use std::{io, thread::sleep};

fn main() {
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
    let peripherals = Peripherals::take().map_err(io::Error::other)?;
    let pins = peripherals.pins;
    let uart = uart::UartDriver::new(
        peripherals.uart2,
        pins.gpio17,
        pins.gpio16,
        Option::<AnyIOPin>::None,
        Option::<AnyIOPin>::None,
        &minitel::esp::default_uart_config(),
    )
    .map_err(io::Error::other)?;
    let mut minitel = minitel::esp::Port::new(uart);

    let sysloop = EspSystemEventLoop::take().map_err(io::Error::other)?;
    let nvs = EspDefaultNvsPartition::take().map_err(io::Error::other)?;
    let mut wifi = EspWifi::new(peripherals.modem, sysloop, Some(nvs)).map_err(io::Error::other)?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))
        .map_err(io::Error::other)?;
    wifi.start().map_err(io::Error::other)?;

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
    App::default().run(&mut minitel, &mut wifi).await?;

    minitel
        .set_routing(true, RoutingRx::Modem, RoutingTx::Keyboard)
        .await
        .unwrap();
    Ok(())
}
