//! Wi-Fi setup and connectivity check for the Minitel.

use esp_idf_svc::{
    ipv4::Ipv4Addr,
    ping::{Configuration as PingConfiguration, EspPing},
    wifi::{AccessPointInfo, AuthMethod, ClientConfiguration, Configuration, EspWifi},
};
use minitel::{
    prelude::*,
    ratatui::MinitelBackend,
    stum::videotex::{C0, FunctionKey, StringMessage, UserInput},
};
use ratatui::{
    prelude::*,
    widgets::{Paragraph, Widget},
};
use std::{
    io::{self, Cursor},
    thread::sleep,
    time::{Duration, Instant},
};

const PING_TARGET: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
const FIRST_PRINTABLE_ASCII: u8 = b' ';
const LAST_PRINTABLE_ASCII: u8 = b'~';

#[derive(Debug)]
pub struct App {
    networks: Vec<AccessPointInfo>,
    selected_network: usize,
    password: String,
    password_visible: bool,
    ascii_picker: bool,
    ascii_code: u8,
    ascii_digits: String,
    status: String,
    exit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            networks: Vec::new(),
            selected_network: 0,
            password: String::new(),
            password_visible: false,
            ascii_picker: false,
            ascii_code: FIRST_PRINTABLE_ASCII,
            ascii_digits: String::new(),
            status: String::new(),
            exit: false,
        }
    }
}

impl App {
    /// Runs the Wi-Fi setup screen until the user quits.
    pub async fn run<B: AsyncMinitelRead + AsyncMinitelWrite>(
        &mut self,
        minitel: &mut B,
        wifi: &mut EspWifi<'_>,
    ) -> io::Result<()> {
        log::info!("Running Wi-Fi setup");
        minitel.send(C0::FF).await?;
        self.scan_networks(wifi);

        let loop_result = self.event_loop(minitel, wifi).await;
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
            self.handle_events(minitel, wifi).await?;
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
            match input {
                UserInput::FunctionKey(FunctionKey::Sommaire) => self.exit = true,
                UserInput::FunctionKey(FunctionKey::Suite) => {
                    if self.ascii_picker {
                        self.step_ascii_code(1);
                    } else if !self.networks.is_empty() {
                        self.selected_network = (self.selected_network + 1) % self.networks.len();
                    }
                }
                UserInput::FunctionKey(FunctionKey::Retour) => {
                    if self.ascii_picker {
                        self.step_ascii_code(-1);
                    } else if !self.networks.is_empty() {
                        self.selected_network = self
                            .selected_network
                            .checked_sub(1)
                            .unwrap_or(self.networks.len() - 1);
                    }
                }
                UserInput::FunctionKey(FunctionKey::Guide) => {
                    self.ascii_picker = !self.ascii_picker;
                    self.ascii_digits.clear();
                }
                UserInput::FunctionKey(FunctionKey::Envoi) => {
                    if self.ascii_picker {
                        self.insert_ascii_character();
                    } else {
                        self.connect_and_ping(wifi);
                    }
                }
                UserInput::FunctionKey(FunctionKey::Correction) => {
                    if self.ascii_picker && !self.ascii_digits.is_empty() {
                        self.ascii_digits.pop();
                    } else {
                        self.password.pop();
                    }
                }
                UserInput::FunctionKey(FunctionKey::Repetition) => {
                    self.password_visible = !self.password_visible;
                }
                UserInput::FunctionKey(FunctionKey::Annulation) => {
                    self.password.clear();
                }
                UserInput::FunctionKey(FunctionKey::ConnexionFin) => {
                    self.scan_networks(wifi);
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
                }
                UserInput::Char(character)
                    if !self.ascii_picker
                        && !character.is_control()
                        && self.password.len() + character.len_utf8() <= 63 =>
                {
                    self.password.push(character);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn scan_networks(&mut self, wifi: &mut EspWifi<'_>) {
        self.status = "Recherche des reseaux Wi-Fi...".to_string();
        match wifi.scan() {
            Ok(networks) => {
                self.networks = networks;
                self.selected_network = 0;
                self.password.clear();
                self.status = if self.networks.is_empty() {
                    "Aucun reseau trouve. Connexion/Fin pour rescanner.".to_string()
                } else {
                    format!("{} reseau(x) trouve(s).", self.networks.len())
                };
            }
            Err(error) => {
                log::error!("Wi-Fi scan failed: {error}");
                self.networks.clear();
                self.selected_network = 0;
                self.status = format!("Echec du scan Wi-Fi: {error}");
            }
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

    fn connect_and_ping(&mut self, wifi: &mut EspWifi<'_>) {
        let Some(network) = self.networks.get(self.selected_network).cloned() else {
            self.status = "Aucun reseau selectionne. Guide pour rescanner.".to_string();
            return;
        };

        let password = match self.password.as_str().try_into() {
            Ok(password) => password,
            Err(_) => {
                self.status = "Mot de passe trop long (63 octets maximum).".to_string();
                return;
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

        self.status = format!("Connexion a {}...", network.ssid);
        if let Err(error) = wifi
            .set_configuration(&config)
            .and_then(|()| wifi.connect())
        {
            log::error!("Wi-Fi connection could not be started: {error}");
            self.status = format!("Impossible de lancer la connexion: {error}");
            return;
        }

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match wifi.is_connected() {
                Ok(true) => match wifi.sta_netif().is_up() {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        log::error!("Could not check Wi-Fi network interface: {error}");
                        self.status = format!("Erreur reseau: {error}");
                        return;
                    }
                },
                Ok(false) => {}
                Err(error) => {
                    log::error!("Could not check Wi-Fi connection: {error}");
                    self.status = format!("Erreur Wi-Fi: {error}");
                    return;
                }
            }

            if Instant::now() >= deadline {
                self.status = format!("Connexion a {} expiree.", network.ssid);
                return;
            }
            sleep(Duration::from_millis(200));
        }

        self.status = format!("Connecte. Ping de {PING_TARGET}...");
        let mut ping = EspPing::new(wifi.sta_netif().get_index());
        match ping.ping(
            PING_TARGET,
            &PingConfiguration {
                count: 3,
                interval: Duration::from_millis(500),
                timeout: Duration::from_secs(1),
                ..Default::default()
            },
        ) {
            Ok(summary) if summary.received > 0 => {
                self.status = format!(
                    "Connecte. Ping OK: {}/{} reponses.",
                    summary.received, summary.transmitted
                );
            }
            Ok(summary) => {
                self.status = format!(
                    "Wi-Fi connecte, ping sans reponse ({}/{}).",
                    summary.received, summary.transmitted
                );
            }
            Err(error) => {
                log::error!("Ping to {PING_TARGET} failed: {error}");
                self.status = format!("Wi-Fi connecte, erreur ping: {error}");
            }
        }
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let section = |offset: u16, height: u16| {
            Rect::new(
                area.x,
                area.y.saturating_add(offset),
                area.width,
                area.height.saturating_sub(offset).min(height),
            )
        };
        let title_area = section(0, 1);
        let networks_title_area = section(1, 1);
        let networks_area = section(2, 14);
        let password_area = section(16, 2);
        let picker_area = section(18, 2);
        let status_area = section(20, 2);
        let instructions_area = section(22, 3);

        Paragraph::new(" Configuration Wi-Fi ")
            .style((Color::Yellow, Color::Black))
            .alignment(Alignment::Center)
            .render(title_area, buf);
        Paragraph::new("Reseaux disponibles (Suite/Retour)")
            .style((Color::White, Color::Blue))
            .render(networks_title_area, buf);

        let list_height = networks_area.height as usize;
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
                        "{marker} {:<24} {:>3} dBm",
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
        Paragraph::new(network_lines).render(networks_area, buf);

        let password_mask = "*".repeat(self.password.chars().count());
        let password_text = if self.password_visible {
            visible_password(&self.password)
        } else {
            password_mask
        };
        Paragraph::new(vec![Line::from("Mot de passe:"), Line::from(password_text)])
            .style((Color::White, Color::Blue))
            .render(password_area, buf);

        let picker_lines = if self.ascii_picker {
            vec![
                Line::from(format!("ASCII {} Suite/Retour +/-", self.ascii_code)),
                Line::from(format!("Code: {} (32-126), Envoi +", self.ascii_digits)),
            ]
        } else {
            vec![
                Line::from("Guide: saisie ASCII (32-126)"),
                Line::from("Repetition: afficher/masquer"),
            ]
        };
        Paragraph::new(picker_lines)
            .style((Color::Cyan, Color::Blue))
            .render(picker_area, buf);

        Paragraph::new(self.status.as_str())
            .style((Color::Yellow, Color::Blue))
            .wrap(ratatui::widgets::Wrap { trim: true })
            .render(status_area, buf);
        let instructions = if self.ascii_picker {
            vec![
                Line::from("Envoi: ajouter  Suite/Retour: code"),
                Line::from("Correction: retour  Guide: fermer"),
                Line::from("Fin: rescanner  Sommaire: quitter"),
            ]
        } else {
            vec![
                Line::from("Envoi: connecter  Suite/Retour: reseaux"),
                Line::from("Correction: retour  Annulation: vider"),
                Line::from("Fin: rescanner  Sommaire: quitter"),
            ]
        };
        Paragraph::new(instructions)
            .style((Color::White, Color::Blue))
            .render(instructions_area, buf);
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
        let mut app = App::default();
        app.ascii_code = FIRST_PRINTABLE_ASCII;
        app.insert_ascii_character();
        app.ascii_code = LAST_PRINTABLE_ASCII;
        app.insert_ascii_character();

        assert_eq!(app.password, " ~");
    }

    #[test]
    fn picker_accepts_decimal_ascii_codes() {
        let mut app = App::default();
        app.ascii_digits = "126".to_string();
        app.insert_ascii_character();

        assert_eq!(app.password, "~");
    }

    #[test]
    fn password_visibility_escapes_characters_not_displayable_on_minitel() {
        assert_eq!(visible_password("abc~"), "abc\\x7E");
        assert_eq!(visible_password("A"), "A");
    }
}
