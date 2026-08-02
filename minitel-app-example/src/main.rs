mod app;

#[cfg(feature = "esp")]
mod main_esp;

#[cfg(feature = "esp")]
mod wifi_esp;

#[cfg(feature = "axum")]
mod main_axum;

#[cfg(feature = "tcp")]
mod main_tcp;

fn main() {
    #[cfg(feature = "esp")]
    crate::main_esp::main();

    #[cfg(feature = "axum")]
    crate::main_axum::main();

    #[cfg(feature = "tcp")]
    crate::main_tcp::main();
}
