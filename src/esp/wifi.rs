//! Wi-Fi setup and connectivity check for the Minitel.

use crate::{
    prelude::*,
    ratatui::MinitelBackend,
    stum::videotex::{C0, FunctionKey, StringMessage, UserInput},
};
use esp_idf_svc::{
    ipv4::Ipv4Addr,
    sys::{
        self, ESP_OK, esp_ping_callbacks_t, esp_ping_config_t, esp_ping_delete_session,
        esp_ping_get_profile, esp_ping_handle_t, esp_ping_new_session,
        esp_ping_profile_t_ESP_PING_PROF_REPLY, esp_ping_profile_t_ESP_PING_PROF_REQUEST,
        esp_ping_start, esp_ping_stop, ip_addr__bindgen_ty_1, ip_addr_t, ip4_addr_t,
        lwip_ip_addr_type_IPADDR_TYPE_V4,
    },
    wifi::{AccessPointInfo, AuthMethod, ClientConfiguration, Configuration, EspWifi},
};
use ratatui::{
    prelude::*,
    widgets::{Paragraph, Widget},
};
use std::{
    io::{self, Cursor},
    sync::atomic::{AtomicBool, AtomicU16, Ordering},
    thread::sleep,
    time::{Duration, Instant},
};

const PING_TARGET: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
const FIRST_PRINTABLE_ASCII: u8 = b' ';
const LAST_PRINTABLE_ASCII: u8 = b'~';
// Leave room for Rust runtime frames in the ESP-IDF ping task.
const PING_TASK_STACK_SIZE: u32 = 8192;

#[derive(Debug)]
pub struct WifiForm {
    page: Page,
    exit: bool,
}

#[derive(Debug)]
enum Page {
    Networks(NetworksPage),
    Password(PasswordPage),
    Connecting(ConnectingPage),
    Result(ResultPage),
}

#[derive(Debug, Default)]
struct NetworksPage {
    networks: Vec<AccessPointInfo>,
    selected_network: usize,
    status: String,
}

#[derive(Debug)]
struct PasswordPage {
    network: AccessPointInfo,
    networks_page: NetworksPage,
    password: String,
    visible: bool,
    ascii_picker: bool,
    ascii_code: u8,
    ascii_digits: String,
    status: String,
}

#[derive(Debug)]
struct ConnectingPage {
    password_page: PasswordPage,
}

#[derive(Debug)]
struct ResultPage {
    password_page: PasswordPage,
    status: String,
}

#[derive(Debug)]
enum PageAction {
    None,
    Exit,
    Rescan,
    SelectNetwork(AccessPointInfo),
    BackToNetworks,
    BackToPassword,
    Connect,
    ShowNetworks,
}

impl Default for WifiForm {
    fn default() -> Self {
        Self {
            page: Page::Networks(NetworksPage::default()),
            exit: false,
        }
    }
}

impl PasswordPage {
    fn new(network: AccessPointInfo, networks_page: NetworksPage) -> Self {
        Self {
            network,
            networks_page,
            password: String::new(),
            visible: false,
            ascii_picker: false,
            ascii_code: FIRST_PRINTABLE_ASCII,
            ascii_digits: String::new(),
            status: String::new(),
        }
    }

    fn handle_input(&mut self, input: UserInput) -> PageAction {
        match input {
            UserInput::FunctionKey(FunctionKey::Sommaire) => PageAction::Exit,
            UserInput::FunctionKey(FunctionKey::Suite) if self.ascii_picker => {
                self.step_ascii_code(1);
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Suite) => PageAction::Connect,
            UserInput::FunctionKey(FunctionKey::Retour) if self.ascii_picker => {
                self.step_ascii_code(-1);
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Retour) => PageAction::BackToNetworks,
            UserInput::FunctionKey(FunctionKey::Guide) => {
                self.ascii_picker = !self.ascii_picker;
                self.ascii_digits.clear();
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Envoi) if self.ascii_picker => {
                self.insert_ascii_character();
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Envoi) => PageAction::Connect,
            UserInput::FunctionKey(FunctionKey::Correction)
                if self.ascii_picker && !self.ascii_digits.is_empty() =>
            {
                self.ascii_digits.pop();
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Correction) => {
                self.password.pop();
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Repetition) => {
                self.visible = !self.visible;
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Annulation) => {
                self.password.clear();
                PageAction::None
            }
            UserInput::Char(character) if self.ascii_picker && character.is_ascii_digit() => {
                if self.ascii_digits.len() < 3 {
                    self.ascii_digits.push(character);
                    if let Ok(code) = self.ascii_digits.parse::<u8>() {
                        if (FIRST_PRINTABLE_ASCII..=LAST_PRINTABLE_ASCII).contains(&code) {
                            self.ascii_code = code;
                        }
                    }
                }
                PageAction::None
            }
            UserInput::Char(character)
                if !self.ascii_picker
                    && !character.is_control()
                    && self.password.len() + character.len_utf8() <= 63 =>
            {
                self.password.push(character);
                PageAction::None
            }
            _ => PageAction::None,
        }
    }

    fn step_ascii_code(&mut self, step: i8) {
        self.ascii_code = if step > 0 {
            if self.ascii_code == LAST_PRINTABLE_ASCII {
                FIRST_PRINTABLE_ASCII
            } else {
                self.ascii_code + 1
            }
        } else if self.ascii_code == FIRST_PRINTABLE_ASCII {
            LAST_PRINTABLE_ASCII
        } else {
            self.ascii_code - 1
        };
        self.ascii_digits.clear();
    }

    fn insert_ascii_character(&mut self) {
        let code = if self.ascii_digits.is_empty() {
            self.ascii_code
        } else {
            match self.ascii_digits.parse::<u8>() {
                Ok(code) if (FIRST_PRINTABLE_ASCII..=LAST_PRINTABLE_ASCII).contains(&code) => code,
                _ => {
                    self.status = format!(
                        "Code ASCII invalide: choisir {}-{}.",
                        FIRST_PRINTABLE_ASCII, LAST_PRINTABLE_ASCII
                    );
                    self.ascii_digits.clear();
                    return;
                }
            }
        };

        if self.password.len() < 63 {
            self.password.push(char::from(code));
            self.ascii_code = code;
        } else {
            self.status = "Mot de passe trop long (63 octets max.).".to_string();
        }
        self.ascii_digits.clear();
    }
}

impl NetworksPage {
    fn handle_input(&mut self, input: UserInput) -> PageAction {
        match input {
            UserInput::FunctionKey(FunctionKey::Sommaire) => PageAction::Exit,
            UserInput::FunctionKey(FunctionKey::Suite) if !self.networks.is_empty() => {
                self.selected_network = (self.selected_network + 1) % self.networks.len();
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Retour) if !self.networks.is_empty() => {
                self.selected_network = self
                    .selected_network
                    .checked_sub(1)
                    .unwrap_or(self.networks.len() - 1);
                PageAction::None
            }
            UserInput::FunctionKey(FunctionKey::Envoi) => self
                .networks
                .get(self.selected_network)
                .cloned()
                .map(PageAction::SelectNetwork)
                .unwrap_or(PageAction::None),
            UserInput::FunctionKey(FunctionKey::ConnexionFin) => PageAction::Rescan,
            _ => PageAction::None,
        }
    }
}

impl ResultPage {
    fn handle_input(&mut self, input: UserInput) -> PageAction {
        match input {
            UserInput::FunctionKey(FunctionKey::Sommaire) => PageAction::Exit,
            UserInput::FunctionKey(FunctionKey::Retour) => PageAction::BackToPassword,
            UserInput::FunctionKey(FunctionKey::Suite | FunctionKey::Envoi) => {
                PageAction::ShowNetworks
            }
            _ => PageAction::None,
        }
    }
}

impl WifiForm {
    /// Runs the Wi-Fi setup screen until the user quits.
    pub async fn run<B: AsyncMinitelRead + AsyncMinitelWrite>(
        &mut self,
        minitel: &mut B,
        wifi: &mut EspWifi<'_>,
        disconnect_reason: &AtomicU16,
    ) -> io::Result<()> {
        log::info!("Running Wi-Fi setup");
        minitel.send(C0::FF).await?;
        self.scan_networks(wifi);

        let loop_result = self.event_loop(minitel, wifi, disconnect_reason).await;
        if let Err(err) = loop_result {
            log::error!("Error in event loop: {:?}", err);
        }
        minitel.send(C0::FF).await?;
        minitel
            .send(StringMessage("Au revoir !".to_string()))
            .await?;

        Ok(())
    }

    async fn event_loop<B: AsyncMinitelRead + AsyncMinitelReadWrite>(
        &mut self,
        minitel: &mut B,
        wifi: &mut EspWifi<'_>,
        disconnect_reason: &AtomicU16,
    ) -> io::Result<()> {
        let cursor = Cursor::new(Vec::new());
        let backend = MinitelBackend::new(cursor);
        let mut terminal = Terminal::new(backend)?;

        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            let cursor = &mut terminal.backend_mut().stream;
            let buffer = cursor.get_mut();
            minitel.write(buffer).await?;
            buffer.clear();
            cursor.set_position(0);
            if let Page::Connecting(connecting) = &self.page {
                let result = Self::connection_result(
                    &connecting.password_page.network,
                    &connecting.password_page.password,
                    wifi,
                    disconnect_reason,
                );
                let Page::Connecting(connecting) =
                    std::mem::replace(&mut self.page, Page::Networks(NetworksPage::default()))
                else {
                    unreachable!();
                };
                self.page = Page::Result(ResultPage {
                    password_page: connecting.password_page,
                    status: result,
                });
            } else {
                self.handle_events(minitel, wifi).await?;
            }
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    async fn handle_events<B: AsyncMinitelRead + AsyncMinitelReadWrite>(
        &mut self,
        minitel: &mut B,
        wifi: &mut EspWifi<'_>,
    ) -> io::Result<()> {
        if let Ok(input) = minitel.read_s0_stroke().await {
            let action = match &mut self.page {
                Page::Networks(page) => page.handle_input(input),
                Page::Password(page) => page.handle_input(input),
                Page::Result(page) => page.handle_input(input),
                Page::Connecting(_) => PageAction::None,
            };

            match action {
                PageAction::None => {}
                PageAction::Exit => self.exit = true,
                PageAction::Rescan => self.scan_networks(wifi),
                PageAction::SelectNetwork(network) => {
                    let Page::Networks(networks_page) =
                        std::mem::replace(&mut self.page, Page::Networks(NetworksPage::default()))
                    else {
                        return Ok(());
                    };
                    self.page = Page::Password(PasswordPage::new(network, networks_page));
                }
                PageAction::BackToNetworks => {
                    if let Page::Password(password_page) =
                        std::mem::replace(&mut self.page, Page::Networks(NetworksPage::default()))
                    {
                        self.page = Page::Networks(password_page.networks_page);
                    };
                }
                PageAction::BackToPassword => {
                    if let Page::Result(result) =
                        std::mem::replace(&mut self.page, Page::Networks(NetworksPage::default()))
                    {
                        self.page = Page::Password(result.password_page);
                    };
                }
                PageAction::ShowNetworks => {
                    if let Page::Result(result) =
                        std::mem::replace(&mut self.page, Page::Networks(NetworksPage::default()))
                    {
                        let mut networks_page = result.password_page.networks_page;
                        networks_page.status.clear();
                        self.page = Page::Networks(networks_page);
                    };
                }
                PageAction::Connect => {
                    let Page::Password(password_page) =
                        std::mem::replace(&mut self.page, Page::Networks(NetworksPage::default()))
                    else {
                        return Ok(());
                    };
                    self.page = Page::Connecting(ConnectingPage { password_page });
                }
            }
        }
        Ok(())
    }

    fn scan_networks(&mut self, wifi: &mut EspWifi<'_>) {
        let mut page = NetworksPage {
            status: "Recherche des reseaux Wi-Fi...".to_string(),
            ..Default::default()
        };
        match wifi.scan() {
            Ok(networks) => {
                page.networks = networks;
                page.status = if page.networks.is_empty() {
                    "Aucun reseau trouve. Connexion/Fin pour rescanner.".to_string()
                } else {
                    format!("{} reseau(x) trouve(s).", page.networks.len())
                };
            }
            Err(error) => {
                log::error!("Wi-Fi scan failed: {error}");
                page.status = format!("Echec du scan Wi-Fi: {error}");
            }
        }
        self.page = Page::Networks(page);
    }

    fn connection_result(
        network: &AccessPointInfo,
        password: &str,
        wifi: &mut EspWifi<'_>,
        disconnect_reason: &AtomicU16,
    ) -> String {
        let password = match password.try_into() {
            Ok(password) => password,
            Err(_) => {
                return "Mot de passe trop long (63 octets maximum).".to_string();
            }
        };
        let config = Configuration::Client(ClientConfiguration {
            ssid: network.ssid.clone(),
            bssid: Some(network.bssid),
            auth_method: network.auth_method.unwrap_or(AuthMethod::WPA2Personal),
            password,
            channel: Some(network.channel),
            ..Default::default()
        });

        disconnect_reason.store(0, Ordering::Relaxed);
        if let Err(error) = wifi
            .set_configuration(&config)
            .and_then(|()| wifi.connect())
        {
            log::error!("Wi-Fi connection could not be started: {error}");
            return format!("Impossible de lancer la connexion: {error}");
        }

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match wifi.is_connected() {
                Ok(true) => match wifi.sta_netif().is_up() {
                    Ok(true) => break,
                    Ok(false) => {
                        let reason = disconnect_reason.load(Ordering::Relaxed);
                        if reason != 0 {
                            return wifi_disconnect_status(reason);
                        }
                    }
                    Err(error) => {
                        log::error!("Could not check Wi-Fi network interface: {error}");
                        return format!("Erreur reseau: {error}");
                    }
                },
                Ok(false) => {
                    let reason = disconnect_reason.load(Ordering::Relaxed);
                    if reason != 0 {
                        return wifi_disconnect_status(reason);
                    }
                }
                Err(error) => {
                    log::error!("Could not check Wi-Fi connection: {error}");
                    return format!("Erreur Wi-Fi: {error}");
                }
            }

            if Instant::now() >= deadline {
                let reason = disconnect_reason.load(Ordering::Relaxed);
                return if reason != 0 {
                    wifi_disconnect_status(reason)
                } else {
                    format!(
                        "Delai depasse pour {} (aucun motif de refus recu).",
                        network.ssid
                    )
                };
            }
            sleep(Duration::from_millis(200));
        }

        match ping_once(wifi.sta_netif().get_index()) {
            Ok(true) => {
                format!("Connexion reussie. Ping de {} reussi.", PING_TARGET)
            }
            Ok(false) => {
                format!("Wi-Fi connecte, aucune reponse de {PING_TARGET}.")
            }
            Err(error) => {
                log::error!("Ping to {PING_TARGET} failed: {error}");
                format!("Wi-Fi connecte, erreur ping: {error}")
            }
        }
    }
}

fn ping_once(interface: u32) -> io::Result<bool> {
    let target_addr = ip_addr_t {
        u_addr: ip_addr__bindgen_ty_1 {
            ip4: ip4_addr_t {
                addr: u32::from_be_bytes(PING_TARGET.octets()),
            },
        },
        type_: lwip_ip_addr_type_IPADDR_TYPE_V4 as u8,
    };
    let config = esp_ping_config_t {
        count: 1,
        interval_ms: 1000,
        timeout_ms: 1000,
        data_size: 56,
        tos: 0,
        ttl: 64,
        target_addr,
        task_stack_size: PING_TASK_STACK_SIZE,
        task_prio: 2,
        interface,
    };
    let completed = AtomicBool::new(false);
    let callbacks = esp_ping_callbacks_t {
        cb_args: &completed as *const AtomicBool as *mut std::ffi::c_void,
        on_ping_success: None,
        on_ping_timeout: None,
        on_ping_end: Some(ping_finished),
    };
    let mut handle: esp_ping_handle_t = std::ptr::null_mut();

    let result = unsafe { esp_ping_new_session(&config, &callbacks, &mut handle) };
    check_ping_result(result)?;
    if handle.is_null() {
        return Err(io::Error::other("ESP-IDF returned an empty ping session"));
    }

    let start_result = unsafe { esp_ping_start(handle) };
    if start_result != ESP_OK {
        let delete_result = unsafe { esp_ping_delete_session(handle) };
        if delete_result != ESP_OK {
            log::error!("Could not delete failed ping session: {delete_result}");
        }
        return Err(io::Error::from_raw_os_error(start_result));
    }

    let deadline = Instant::now() + Duration::from_secs(4);
    while !completed.load(Ordering::Acquire) && Instant::now() < deadline {
        sleep(Duration::from_millis(20));
    }

    if !completed.load(Ordering::Acquire) {
        let stop_result = unsafe { esp_ping_stop(handle) };
        if stop_result != ESP_OK {
            log::error!("Could not stop timed-out ping session: {stop_result}");
        }
        let delete_result = unsafe { esp_ping_delete_session(handle) };
        if delete_result != ESP_OK {
            log::error!("Could not delete timed-out ping session: {delete_result}");
        }
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "ESP-IDF ping session did not finish",
        ));
    }

    let mut replies = 0u32;
    let mut requests = 0u32;
    let reply_result = unsafe {
        esp_ping_get_profile(
            handle,
            esp_ping_profile_t_ESP_PING_PROF_REPLY,
            &mut replies as *mut u32 as *mut std::ffi::c_void,
            std::mem::size_of_val(&replies) as u32,
        )
    };
    let request_result = unsafe {
        esp_ping_get_profile(
            handle,
            esp_ping_profile_t_ESP_PING_PROF_REQUEST,
            &mut requests as *mut u32 as *mut std::ffi::c_void,
            std::mem::size_of_val(&requests) as u32,
        )
    };
    let stop_result = unsafe { esp_ping_stop(handle) };
    let delete_result = unsafe { esp_ping_delete_session(handle) };

    check_ping_result(reply_result)?;
    check_ping_result(request_result)?;
    check_ping_result(stop_result)?;
    check_ping_result(delete_result)?;

    Ok(replies > 0 && requests > 0)
}

unsafe extern "C" fn ping_finished(_handle: esp_ping_handle_t, args: *mut std::ffi::c_void) {
    if !args.is_null() {
        let completed = unsafe { &*(args as *const AtomicBool) };
        completed.store(true, Ordering::Release);
    }
}

fn check_ping_result(result: sys::esp_err_t) -> io::Result<()> {
    if result == ESP_OK {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

fn wifi_disconnect_status(reason: u16) -> String {
    use esp_idf_svc::sys::{
        wifi_err_reason_t_WIFI_REASON_4WAY_HANDSHAKE_TIMEOUT,
        wifi_err_reason_t_WIFI_REASON_AUTH_FAIL, wifi_err_reason_t_WIFI_REASON_HANDSHAKE_TIMEOUT,
        wifi_err_reason_t_WIFI_REASON_NO_AP_FOUND,
    };

    if reason == wifi_err_reason_t_WIFI_REASON_AUTH_FAIL as u16 {
        "Authentification refusee: mot de passe incorrect ou securite incompatible.".to_string()
    } else if reason == wifi_err_reason_t_WIFI_REASON_4WAY_HANDSHAKE_TIMEOUT as u16
        || reason == wifi_err_reason_t_WIFI_REASON_HANDSHAKE_TIMEOUT as u16
    {
        "Echec de la negociation Wi-Fi: mot de passe incorrect possible.".to_string()
    } else if reason == wifi_err_reason_t_WIFI_REASON_NO_AP_FOUND as u16 {
        "Reseau introuvable: aucun point d'acces correspondant.".to_string()
    } else {
        format!("Connexion Wi-Fi interrompue (motif {reason}).")
    }
}

impl Widget for &WifiForm {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match &self.page {
            Page::Networks(page) => page.render(area, buf),
            Page::Password(page) => page.render(area, buf),
            Page::Connecting(page) => page.render(area, buf),
            Page::Result(page) => page.render(area, buf),
        }
    }
}

impl Page {
    fn draw_frame(area: Rect, buf: &mut Buffer) -> (Rect, Rect) {
        Paragraph::new(" Configuration Wi-Fi ")
            .style((Color::Yellow, Color::Black))
            .alignment(Alignment::Center)
            .render(
                Rect::new(area.x, area.y, area.width, 1.min(area.height)),
                buf,
            );

        let content = Rect::new(
            area.x,
            area.y.saturating_add(2),
            area.width,
            area.height.saturating_sub(7),
        );
        let footer = Rect::new(
            area.x,
            area.y.saturating_add(area.height.saturating_sub(3)),
            area.width,
            area.height.min(3),
        );
        (content, footer)
    }

    fn status_area(area: Rect) -> Rect {
        Rect::new(
            area.x,
            area.y.saturating_add(area.height.saturating_sub(5)),
            area.width,
            2.min(area.height.saturating_sub(3)),
        )
    }
}

const NETWORKS_PAGE_FOOTER: &str = concat![
    "Retour/Suite: choisir     Envoi: valider\n",
    "Fin: rescanner         Sommaire: quitter"
];

impl NetworksPage {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let (content, footer) = Page::draw_frame(area, buf);
        Paragraph::new("Choisissez un reseau")
            .style((Color::White, Color::Blue))
            .render(
                Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
                buf,
            );

        let list_height = content.height as usize;
        let first_visible = self
            .selected_network
            .saturating_sub(list_height.saturating_sub(1));
        let network_lines = if self.networks.is_empty() {
            vec![Line::from("Aucun reseau disponible")]
        } else {
            self.networks
                .iter()
                .enumerate()
                .skip(first_visible)
                .take(list_height)
                .map(|(index, network)| {
                    let marker = if index == self.selected_network {
                        ">"
                    } else {
                        " "
                    };
                    let line = Line::from(format!(
                        "{marker} {:<28} {:>3} dBm",
                        network.ssid, network.signal_strength
                    ));
                    if index == self.selected_network {
                        line.style((Color::Black, Color::Yellow))
                    } else {
                        line
                    }
                })
                .collect()
        };
        Paragraph::new(network_lines).render(content, buf);
        Paragraph::new(self.status.as_str())
            .style((Color::Yellow, Color::Blue))
            .wrap(ratatui::widgets::Wrap { trim: true })
            .render(Page::status_area(area), buf);
        Paragraph::new(NETWORKS_PAGE_FOOTER)
            .style((Color::White, Color::Blue))
            .render(footer, buf);
    }
}

const PASSWORD_PAGE_FOOTER: &str = concat![
    "Retour: reseaux   Suite/Envoi: connecter\n",
    "Correction: effacer    Annulation: vider\n",
    "Guide: ASCII        Repetition: afficher"
];
const PASSWORD_PAGE_ASCII_PICKER: &str = concat![
    "Suite/Retour: caractere   Envoi: ajouter\n",
    "Correction: effacer code   Guide: fermer\n",
    "Sommaire: quitter"
];

impl PasswordPage {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let (content, footer) = Page::draw_frame(area, buf);
        Paragraph::new(vec![
            Line::from("Reseau selectionne:"),
            Line::from(self.network.ssid.as_str()).style((Color::Yellow, Color::Blue)),
            Line::from("Mot de passe:"),
            Line::from(if self.visible {
                visible_password(&self.password)
            } else {
                "*".repeat(self.password.chars().count())
            }),
        ])
        .style((Color::White, Color::Blue))
        .render(content, buf);

        if self.ascii_picker {
            Paragraph::new(vec![
                Line::from(format!("ASCII {}  Suite/Retour: +/-", self.ascii_code)),
                Line::from(format!(
                    "Code: {} (32-126), Envoi: ajouter",
                    self.ascii_digits
                )),
            ])
            .style((Color::Cyan, Color::Blue))
            .render(Page::status_area(area), buf);
        } else if !self.status.is_empty() {
            Paragraph::new(self.status.as_str())
                .style((Color::Yellow, Color::Blue))
                .wrap(ratatui::widgets::Wrap { trim: true })
                .render(Page::status_area(area), buf);
        }
        let instructions = if self.ascii_picker {
            PASSWORD_PAGE_ASCII_PICKER
        } else {
            PASSWORD_PAGE_FOOTER
        };
        Paragraph::new(instructions)
            .style((Color::White, Color::Blue))
            .render(footer, buf);
    }
}

impl ConnectingPage {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let (content, footer) = Page::draw_frame(area, buf);
        Paragraph::new("Connexion au reseau...\nVeuillez patienter.")
            .style((Color::Yellow, Color::Blue))
            .alignment(Alignment::Center)
            .wrap(ratatui::widgets::Wrap { trim: true })
            .render(content, buf);
        Paragraph::new("Connexion en cours...")
            .style((Color::White, Color::Blue))
            .render(footer, buf);
    }
}

const RESULT_PAGE_FOOTER: &str = concat![
    "Retour: mot de passe  Suite: reseaux\n",
    "Envoi: reseaux     Sommaire: quitter"
];

impl ResultPage {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let (content, footer) = Page::draw_frame(area, buf);
        Paragraph::new(self.status.as_str())
            .style((Color::Yellow, Color::Blue))
            .alignment(Alignment::Center)
            .wrap(ratatui::widgets::Wrap { trim: true })
            .render(content, buf);
        Paragraph::new(RESULT_PAGE_FOOTER)
            .style((Color::White, Color::Blue))
            .render(footer, buf);
    }
}

fn visible_password(password: &str) -> String {
    password
        .chars()
        .map(|character| match character {
            ' '..=']' | '_' | 'a'..='z' | '|' => character.to_string(),
            character if character.is_ascii() => format!("\\x{:02X}", character as u8),
            character => format!("\\u{{{:X}}}", character as u32),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_inserts_printable_ascii_at_both_ends_of_the_range() {
        let mut page = PasswordPage::new(test_network(), NetworksPage::default());
        page.ascii_code = FIRST_PRINTABLE_ASCII;
        page.insert_ascii_character();
        page.ascii_code = LAST_PRINTABLE_ASCII;
        page.insert_ascii_character();

        assert_eq!(page.password, " ~");
    }

    #[test]
    fn picker_accepts_decimal_ascii_codes() {
        let mut page = PasswordPage::new(test_network(), NetworksPage::default());
        page.ascii_digits = "126".to_string();
        page.insert_ascii_character();

        assert_eq!(page.password, "~");
    }

    #[test]
    fn password_visibility_escapes_characters_not_displayable_on_minitel() {
        assert_eq!(visible_password("abc~"), "abc\\x7E");
        assert_eq!(visible_password("A"), "A");
    }

    #[test]
    fn network_list_uses_up_and_down_arrows_for_selection() {
        let mut page = NetworksPage {
            networks: vec![test_network(), test_network()],
            ..Default::default()
        };

        page.handle_input(UserInput::C0(C0::LF));
        assert_eq!(page.selected_network, 1);

        page.handle_input(UserInput::C0(C0::VT));
        assert_eq!(page.selected_network, 0);
    }

    #[test]
    fn suite_selects_the_current_network() {
        let network = test_network();
        let mut page = NetworksPage {
            networks: vec![network.clone()],
            ..Default::default()
        };

        assert!(matches!(
            page.handle_input(UserInput::FunctionKey(FunctionKey::Suite)),
            PageAction::SelectNetwork(selected) if selected == network
        ));
    }

    fn test_network() -> AccessPointInfo {
        AccessPointInfo {
            ssid: "test".try_into().unwrap(),
            ..Default::default()
        }
    }
}
