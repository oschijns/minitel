#[doc(inline)]
pub use esp::*;

#[cfg(feature = "esp")]
mod esp {
    use crate::{AsyncMinitelBaudrateControl, AsyncMinitelRead, AsyncMinitelWrite};
    use esp_idf_hal::{
        delay,
        gpio::AnyIOPin,
        sys::{ESP_ERR_TIMEOUT, EspError},
        task::yield_now,
        uart,
        units::Hertz,
    };
    use std::{
        borrow::BorrowMut,
        io::{Error, ErrorKind, Result},
    };

    /// Serial port configuration when the minitel starts
    pub fn default_uart_config() -> uart::UartConfig {
        uart::UartConfig::default()
            .baudrate(Hertz(1200))
            .stop_bits(uart::config::StopBits::STOP1)
            .data_bits(uart::config::DataBits::DataBits7)
            .parity_even()
    }

    /// Create a new Minitel instance using the port UART 2.
    ///
    /// This is the port used in the ESP32 minitel development board from iodeo.
    ///
    /// Also returns the leftover `modem` peripheral (Wi-Fi/BT radio), which this function
    /// doesn't use itself, so callers can set up Wi-Fi separately.
    #[allow(clippy::type_complexity)]
    pub fn esp_minitel_uart2() -> core::result::Result<
        (
            Port<'static, uart::UartDriver<'static>>,
            esp_idf_hal::modem::Modem<'static>,
        ),
        EspError,
    > {
        let peripherals = esp_idf_hal::peripherals::Peripherals::take()?;
        let pins = peripherals.pins;

        let uart: uart::UartDriver<'static> = uart::UartDriver::new(
            peripherals.uart2,
            pins.gpio17,
            pins.gpio16,
            Option::<AnyIOPin>::None,
            Option::<AnyIOPin>::None,
            &default_uart_config(),
        )?;

        Ok((Port::new(uart), peripherals.modem))
    }

    /// A Minitel serial port backed directly by `esp-idf-hal`'s blocking `UartDriver`.
    ///
    /// Reads/writes/flushes are implemented by polling the driver with a non-blocking
    /// timeout and yielding to the executor between attempts, rather than through
    /// `esp-idf-hal`'s `AsyncUartDriver`. The latter spawns a background FreeRTOS task with a
    /// fixed 2 KB stack that wakes the calling task's `Waker` directly from that stack; under
    /// Tokio's `current_thread` runtime, waking from a different OS task takes Tokio's heavier
    /// cross-thread wake path, which overflows that stack. Busy-polling (the same pattern
    /// `AsyncUartDriver` itself uses for writes, since the UART ISR can't notify on TX space)
    /// avoids the background task and its wake path entirely.
    pub struct Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        pub uart: T,
        _lifetime: core::marker::PhantomData<&'a ()>,
    }

    impl<'a, T> Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        pub fn new(uart: T) -> Self {
            Port {
                uart,
                _lifetime: core::marker::PhantomData,
            }
        }
    }

    impl<'a, T> AsyncMinitelRead for Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        async fn read(&mut self, data: &mut [u8]) -> Result<()> {
            let mut filled = 0;
            while filled < data.len() {
                match self
                    .uart
                    .borrow_mut()
                    .read(&mut data[filled..], delay::NON_BLOCK)
                {
                    Ok(len) if len > 0 => filled += len,
                    Err(e) if e.code() != ESP_ERR_TIMEOUT => {
                        return Err(Error::new(ErrorKind::Other, e));
                    }
                    _ => yield_now().await,
                }
            }
            Ok(())
        }
    }

    impl<'a, T> AsyncMinitelWrite for Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        async fn write(&mut self, data: &[u8]) -> Result<()> {
            let mut written = 0;
            while written < data.len() {
                match self.uart.borrow_mut().write_nb(&data[written..]) {
                    Ok(len) if len > 0 => written += len,
                    Ok(_) => yield_now().await,
                    Err(e) => return Err(Error::new(ErrorKind::Other, e)),
                }
            }
            Ok(())
        }

        async fn flush(&mut self) -> Result<()> {
            loop {
                match self.uart.borrow_mut().wait_tx_done(delay::NON_BLOCK) {
                    Ok(()) => return Ok(()),
                    Err(e) if e.code() != ESP_ERR_TIMEOUT => {
                        return Err(Error::new(ErrorKind::Other, e));
                    }
                    _ => yield_now().await,
                }
            }
        }
    }

    impl<'a, T> AsyncMinitelBaudrateControl for Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        fn set_baudrate(&mut self, baudrate: crate::stum::protocol::Baudrate) -> Result<()> {
            self.uart
                .borrow_mut()
                .change_baudrate(baudrate.hertz())
                .map_err(|e| Error::new(ErrorKind::Other, e))?;
            Ok(())
        }

        fn read_byte_blocking(&mut self) -> Result<u8> {
            let mut byte: [u8; 1] = [0];
            self.uart
                .borrow_mut()
                .read(&mut byte, 20)
                .map_err(|e| Error::new(ErrorKind::Other, e))?;
            Ok(byte[0])
        }
    }
}

/// Doc shenanigans: stubs for ESP32 integration documentation when the ESP toolchain is not available
#[cfg(feature = "espdoc")]
mod esp {
    use std::borrow::BorrowMut;
    use std::io::Result;

    use crate::{AsyncMinitelBaudrateControl, AsyncMinitelRead, AsyncMinitelWrite};

    #[doc(hidden)]
    pub mod uart {
        pub struct UartConfig;

        pub struct UartDriver<'a> {
            _phantom: core::marker::PhantomData<&'a ()>,
        }
    }
    #[doc(hidden)]
    pub mod modem {
        pub struct Modem<'a> {
            _phantom: core::marker::PhantomData<&'a ()>,
        }
    }
    #[doc(hidden)]
    pub struct EspError;

    /// Serial port configuration when the minitel starts
    pub fn default_uart_config() -> uart::UartConfig {
        unimplemented!()
    }

    /// Create a new Minitel instance using the port UART 2.
    ///
    /// This is the port used in the ESP32 minitel development board from iodeo.
    ///
    /// Also returns the leftover `modem` peripheral (Wi-Fi/BT radio), which this function
    /// doesn't use itself, so callers can set up Wi-Fi separately.
    #[allow(clippy::type_complexity)]
    pub fn esp_minitel_uart2() -> core::result::Result<
        (
            Port<'static, uart::UartDriver<'static>>,
            modem::Modem<'static>,
        ),
        EspError,
    > {
        unimplemented!()
    }

    pub struct Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        pub uart: T,
        _lifetime: core::marker::PhantomData<&'a ()>,
    }

    impl<'a, T> Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        pub fn new(uart: T) -> Self {
            Port {
                uart,
                _lifetime: core::marker::PhantomData,
            }
        }
    }

    impl<'a, T> AsyncMinitelRead for Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        async fn read(&mut self, _data: &mut [u8]) -> Result<()> {
            unimplemented!()
        }
    }

    impl<'a, T> AsyncMinitelWrite for Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        async fn write(&mut self, _data: &[u8]) -> Result<()> {
            unimplemented!()
        }

        async fn flush(&mut self) -> Result<()> {
            unimplemented!()
        }
    }

    impl<'a, T> AsyncMinitelBaudrateControl for Port<'a, T>
    where
        T: BorrowMut<uart::UartDriver<'a>>,
    {
        fn set_baudrate(&mut self, _baudrate: crate::stum::protocol::Baudrate) -> Result<()> {
            unimplemented!()
        }

        fn read_byte_blocking(&mut self) -> Result<u8> {
            unimplemented!()
        }
    }
}
