//! Application logic for the Minitel app example.

use minitel::{
    prelude::*,
    ratatui::{MinitelBackend, widgets::Fill},
    stum::videotex::{C0, FunctionKey, StringMessage, UserInput},
};
use ratatui::{
    layout::Flex,
    prelude::*,
    style::Styled,
    symbols::border,
    widgets::{
        Block, Padding, Paragraph, Tabs, Widget, Wrap,
        calendar::{CalendarEventStore, Monthly},
        canvas::{Canvas, Map, MapResolution},
    },
};
use std::io::{self, Cursor};
use strum::{Display, EnumIter, FromRepr, IntoEnumIterator};
use time::{Date, Duration, Month};
use tui_big_text::{BigText, PixelSize};

/// Performs the actual Wi-Fi connection.
///
/// Implemented per-backend: the ESP32 build drives real Wi-Fi hardware through
/// `esp-idf-svc`, while the other backends (tcp/axum) have no radio to control and fall back
/// to [`NoWifiConnector`].
#[allow(async_fn_in_trait)]
pub trait WifiConnector {
    /// Attempt to join the given network, returning the assigned IP address on success.
    async fn connect(&mut self, ssid: &str, password: &str) -> Result<String, String>;
}

/// Stub connector used on backends that don't drive real Wi-Fi hardware.
#[derive(Debug, Default)]
pub struct NoWifiConnector;

impl WifiConnector for NoWifiConnector {
    async fn connect(&mut self, _ssid: &str, _password: &str) -> Result<String, String> {
        Err("Wifi indisponible sur ce serveur".to_string())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum WifiField {
    #[default]
    Ssid,
    Password,
}

#[derive(Debug, Default)]
enum WifiStatus {
    #[default]
    Idle,
    Connecting,
    Connected(String),
    Failed(String),
}

/// State of the Wi-Fi connection form.
#[derive(Debug, Default)]
struct WifiForm {
    ssid: String,
    password: String,
    focus: WifiField,
    status: WifiStatus,
    /// Set when the form was just submitted: the actual (potentially slow) connection attempt
    /// is deferred to the next loop iteration, so the "Connexion en cours..." status has a
    /// chance to be drawn and flushed to the minitel first.
    connect_pending: bool,
}

impl WifiForm {
    fn field_mut(&mut self) -> &mut String {
        match self.focus {
            WifiField::Ssid => &mut self.ssid,
            WifiField::Password => &mut self.password,
        }
    }

    fn push_char(&mut self, c: char) {
        // SSID/password are capped to the sizes esp-idf-svc's ClientConfiguration accepts
        // (32 / 64 bytes); reject control characters, which aren't valid in either.
        if c.is_control() {
            return;
        }
        let max_len = match self.focus {
            WifiField::Ssid => 32,
            WifiField::Password => 64,
        };
        let field = self.field_mut();
        if field.len() + c.len_utf8() <= max_len {
            field.push(c);
        }
    }

    fn backspace(&mut self) {
        self.field_mut().pop();
    }

    fn reset(&mut self) {
        *self = WifiForm::default();
    }
}

/// Application state
#[derive(Debug)]
pub struct App<W: WifiConnector = NoWifiConnector> {
    selected_tab: SelectedTab,
    date: Date,
    demo_disjoint: bool,
    exit: bool,
    wifi: WifiForm,
    wifi_connector: W,
}

impl<W: WifiConnector + Default> Default for App<W> {
    fn default() -> Self {
        Self::with_wifi_connector(W::default())
    }
}

impl<W: WifiConnector> App<W> {
    /// Build an app with an explicit Wi-Fi connector, for connectors that aren't `Default`
    /// (e.g. the ESP32 one, which wraps hardware handles obtained at startup).
    pub fn with_wifi_connector(wifi_connector: W) -> Self {
        Self {
            selected_tab: SelectedTab::default(),
            date: Date::from_calendar_date(2025, Month::January, 15).unwrap(),
            demo_disjoint: false,
            exit: false,
            wifi: WifiForm::default(),
            wifi_connector,
        }
    }

    /// runs the application's main loop until the user quits
    pub async fn run<B: AsyncMinitelRead + AsyncMinitelWrite>(
        &mut self,
        minitel: &mut B,
    ) -> io::Result<()> {
        log::info!("Running App");
        minitel.send(C0::FF).await?;

        let loop_result = self.event_loop(minitel).await;
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
    ) -> io::Result<()> {
        // Prepare a write buffer for the sync->async bridge
        let buffer: Vec<u8> = Vec::new();
        let cursor: Cursor<Vec<u8>> = Cursor::new(buffer);
        let backend = MinitelBackend::new(cursor);
        let mut terminal = Terminal::new(backend)?;
        while !self.exit {
            // Draw the frame to the buffer
            terminal.draw(|frame| self.draw(frame))?;
            // Flush the buffer to the minitel
            let cursor = &mut terminal.backend_mut().stream;
            let buffer = cursor.get_mut();
            minitel.write(buffer).await?;
            buffer.clear();
            cursor.set_position(0);

            if self.wifi.connect_pending {
                // The "Connexion en cours..." status above has now been flushed to the
                // minitel; it's safe to perform the (potentially slow) connection attempt.
                self.wifi.connect_pending = false;
                let result = self
                    .wifi_connector
                    .connect(&self.wifi.ssid, &self.wifi.password)
                    .await;
                self.wifi.status = match result {
                    Ok(ip) => WifiStatus::Connected(ip),
                    Err(err) => WifiStatus::Failed(err),
                };
                continue;
            }

            // Read the minitel input
            self.handle_events(minitel).await?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    async fn handle_events<B: AsyncMinitelRead + AsyncMinitelReadWrite>(
        &mut self,
        minitel: &mut B,
    ) -> io::Result<()> {
        if let Ok(b) = minitel.read_s0_stroke().await {
            match b {
                UserInput::FunctionKey(FunctionKey::Suite) => {
                    self.selected_tab = self.selected_tab.next()
                }
                UserInput::FunctionKey(FunctionKey::Retour) => {
                    self.selected_tab = self.selected_tab.previous()
                }
                UserInput::FunctionKey(FunctionKey::Sommaire) => self.exit = true,
                _ => match self.selected_tab {
                    SelectedTab::Calendrier => match b {
                        UserInput::FunctionKey(FunctionKey::Correction) => {
                            self.date = self.date.saturating_add(Duration::days(20));
                            self.date = self.date.replace_day(15).unwrap();
                        }
                        UserInput::FunctionKey(FunctionKey::Annulation) => {
                            self.date = self.date.saturating_sub(Duration::days(20));
                            self.date = self.date.replace_day(15).unwrap();
                        }
                        _ => {}
                    },
                    SelectedTab::Borders | SelectedTab::World => {
                        if let UserInput::FunctionKey(FunctionKey::Envoi) = b {
                            self.demo_disjoint = !self.demo_disjoint;
                        }
                    }
                    SelectedTab::Wifi => match b {
                        UserInput::Char(c) => self.wifi.push_char(c),
                        UserInput::FunctionKey(FunctionKey::Correction) => self.wifi.backspace(),
                        UserInput::FunctionKey(FunctionKey::Annulation) => self.wifi.reset(),
                        UserInput::FunctionKey(FunctionKey::Envoi) => match self.wifi.focus {
                            WifiField::Ssid => self.wifi.focus = WifiField::Password,
                            WifiField::Password => {
                                self.wifi.status = WifiStatus::Connecting;
                                self.wifi.connect_pending = true;
                            }
                        },
                        _ => {}
                    },
                    _ => {}
                },
            }
        }
        Ok(())
    }
}

#[derive(Default, Clone, Copy, Debug, Display, FromRepr, EnumIter)]
enum SelectedTab {
    #[default]
    #[strum(to_string = "Bienvenue")]
    Bienvenue,
    #[strum(to_string = "Cal")]
    Calendrier,
    #[strum(to_string = "Monde")]
    World,
    #[strum(to_string = "Bordures")]
    Borders,
    #[strum(to_string = "WiFi")]
    Wifi,
}

impl<W: WifiConnector> Widget for &App<W> {
    /// Draw the application to the ratatui buffer
    fn render(self, area: Rect, buf: &mut Buffer) {
        let [title_area, tabs_area, main_area, instructions_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .areas(area);

        Paragraph::new(" Minitel App Example ")
            .style((Color::Yellow, Color::Black))
            .alignment(Alignment::Center)
            .render(title_area, buf);

        self.draw_tabs(buf, tabs_area, main_area);

        match self.selected_tab {
            SelectedTab::Bienvenue => {
                self.draw_welcome(buf, main_area);
            }
            SelectedTab::Calendrier => {
                self.draw_calendar(buf, main_area);
            }
            SelectedTab::World => {
                self.draw_world(buf, main_area);
            }
            SelectedTab::Borders => {
                self.draw_border_demo(buf, main_area);
            }
            SelectedTab::Wifi => {
                self.draw_wifi_form(buf, main_area);
            }
        }

        self.draw_instructions(buf, instructions_area);
    }
}

impl<W: WifiConnector> App<W> {
    fn draw_tabs(&self, buf: &mut Buffer, tabs_area: Rect, main_area: Rect) {
        let titles = SelectedTab::iter().map(SelectedTab::title);
        let selected_tab_index = self.selected_tab as usize;
        Tabs::new(titles)
            .select(selected_tab_index)
            .highlight_style(Style::default())
            .padding("", "")
            .divider(" ")
            .render(tabs_area, buf);

        Block::default()
            .bg(self.selected_tab.color())
            .render(main_area, buf);
    }

    fn draw_welcome(&self, buf: &mut Buffer, main_area: Rect) {
        let big_text_area = vcenter(main_area, Constraint::Length(10));
        BigText::builder()
            .pixel_size(PixelSize::Sextant)
            .style(Style::default().patch((Color::Blue, self.selected_tab.color())))
            .lines(vec![
                "Ratatui".slow_blink().into(),
                "dans ton".into(),
                "Minitel !".underlined().crossed_out().into(),
            ])
            .centered()
            .build()
            .render(big_text_area, buf);
    }

    fn draw_calendar(&self, buf: &mut Buffer, main_area: Rect) {
        let calendar_area = center(main_area, Constraint::Length(23), Constraint::Max(9));
        let calendar_block = Block::bordered()
            .border_set(QUADRANT_OUTSIDE_TOP_FULL)
            .title(calendrier_title(self.date))
            .title_alignment(Alignment::Center)
            .style((Color::Blue, Color::White));
        let [weekdays_area, days_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)])
                .areas(calendar_block.inner(calendar_area));
        calendar_block.render(calendar_area, buf);
        Fill::default().fg(Color::White).render(days_area, buf); // calendar does not draw a background
        Paragraph::new(" Di Lu Ma Me Je Ve Sa ".fg(Color::Magenta).underlined())
            .render(weekdays_area, buf);
        Monthly::new(self.date, CalendarEventStore::default())
            .default_style((Color::Blue, Color::White))
            .show_surrounding(Style::default().fg(Color::Cyan))
            .render(days_area, buf);
    }

    fn draw_world(&self, buf: &mut Buffer, main_area: Rect) {
        Canvas::default()
            .paint(|ctx| {
                ctx.draw(&Map {
                    color: Color::Green,
                    resolution: MapResolution::High,
                });
            })
            .background_color(self.selected_tab.color())
            .x_bounds([-180.0, 180.0])
            .y_bounds([-90.0, 90.0])
            .render(main_area, buf);
        buf.set_style(main_area, Style::default().crossed_out());
        // Force semi-graphic mode
        if self.demo_disjoint {
            buf.set_style(main_area, Style::default().underlined());
        }
    }

    fn draw_border_demo(&self, buf: &mut Buffer, main_area: Rect) {
        let [h1, h2] = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
            .spacing(1)
            .margin(1)
            .areas(main_area);
        let [l11, l12, l13] = Layout::vertical([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .spacing(1)
        .areas(h1);

        let [l21, l22, l23] = Layout::vertical([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .spacing(1)
        .areas(h2);

        let mut border_style = Style::default();
        if self.demo_disjoint {
            border_style = border_style.underlined();
        }
        border_demo(
            " Full ",
            "Bordure pleine",
            border::FULL,
            border_style.patch((Color::Black, Color::Green)),
        )
        .render(l11, buf);
        border_demo(
            " Quad Inside ",
            "Quadrants intérieur",
            border::QUADRANT_INSIDE,
            border_style.patch((Color::Black, Color::Green)),
        )
        .render(l12, buf);
        border_demo(
            " Quad Outside ",
            "Quadrants extérieur",
            border::QUADRANT_OUTSIDE,
            border_style.patch((Color::Black, Color::Cyan)),
        )
        .render(l13, buf);

        border_demo(
            " 8th Width ",
            "Largeur 1/8",
            border::ONE_EIGHTH_WIDE,
            border_style.patch((Color::Black, Color::Green)),
        )
        .render(l21, buf);

        border_demo(
            " 8th Width bis ",
            "Largeur 1/8 décalée",
            minitel::ratatui::border::ONE_EIGHTH_WIDE_OFFSET,
            border_style.patch((Color::Black, Color::Green)),
        )
        .render(l22, buf);

        border_demo(
            " beveled ",
            "Largeur 1/8 biseautée",
            minitel::ratatui::border::ONE_EIGHTH_WIDE_BEVEL,
            border_style.patch((Color::Black, Color::Green)),
        )
        .render(l23, buf);
    }

    fn draw_wifi_form(&self, buf: &mut Buffer, main_area: Rect) {
        let form_area = center(main_area, Constraint::Length(34), Constraint::Length(7));
        let block = Block::bordered()
            .title(" Connexion Wi-Fi ")
            .title_alignment(Alignment::Center)
            .style((Color::White, Color::Blue));
        let inner = block.inner(form_area);
        block.render(form_area, buf);

        let [ssid_area, password_area, _spacer, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        self.draw_wifi_field(
            buf,
            ssid_area,
            "SSID",
            &self.wifi.ssid,
            WifiField::Ssid,
            false,
        );
        self.draw_wifi_field(
            buf,
            password_area,
            "Mot de passe",
            &self.wifi.password,
            WifiField::Password,
            true,
        );

        let (status_text, status_style) = match &self.wifi.status {
            WifiStatus::Idle => (String::new(), Style::default()),
            WifiStatus::Connecting => (
                "Connexion en cours...".to_string(),
                Style::default().fg(Color::Yellow),
            ),
            WifiStatus::Connected(ip) => (
                format!("Connecte : {ip}"),
                Style::default().fg(Color::Green),
            ),
            WifiStatus::Failed(err) => (format!("Echec : {err}"), Style::default().fg(Color::Red)),
        };
        Paragraph::new(status_text)
            .style(status_style)
            .wrap(Wrap { trim: false })
            .render(status_area, buf);
    }

    fn draw_wifi_field(
        &self,
        buf: &mut Buffer,
        area: Rect,
        label: &str,
        value: &str,
        field: WifiField,
        mask: bool,
    ) {
        let displayed = if mask {
            "*".repeat(value.chars().count())
        } else {
            value.to_string()
        };
        let style = if self.wifi.focus == field {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        Paragraph::new(format!("{label}: {displayed}"))
            .style(style)
            .render(area, buf);
    }

    fn draw_instructions(&self, buf: &mut Buffer, instructions_area: Rect) {
        let instructions_1 = Line::from(vec![
            " Onglets:".into(),
            " Suite/Retour".reversed(),
            " Quitter:".into(),
            " Sommaire".reversed(),
        ]);

        let instructions_2 = match self.selected_tab {
            SelectedTab::Calendrier => {
                Line::from(vec![" Mois:".into(), " Correction/Annulation".reversed()])
            }
            SelectedTab::Borders | SelectedTab::World => {
                Line::from(vec![" Joint/Disjoint:".into(), " Envoi".reversed()])
            }
            SelectedTab::Wifi => Line::from(vec![
                " Suivant/OK:".into(),
                " Envoi".reversed(),
                " Effacer:".into(),
                " Correction".reversed(),
            ]),
            _ => Line::default(),
        };

        Paragraph::new(vec![instructions_1, instructions_2])
            .style((Color::Yellow, Color::Blue))
            .render(instructions_area, buf);
    }
}

fn border_demo<'a>(
    name: &'a str,
    content: &'a str,
    border_set: border::Set<'a>,
    border_style: Style,
) -> Paragraph<'a> {
    let block = Block::bordered()
        .border_set(border_set)
        .border_style(border_style)
        .title_alignment(Alignment::Right)
        .title(name.set_style((Color::Yellow, Color::Black)))
        .padding(Padding::left(1));

    Paragraph::new(content)
        .style((Color::Blue, Color::Cyan))
        .wrap(Wrap { trim: false })
        .block(block)
}

fn calendrier_title(date: Date) -> Line<'static> {
    let month = match date.month() {
        Month::January => "Janvier",
        Month::February => "Février",
        Month::March => "Mars",
        Month::April => "Avril",
        Month::May => "Mai",
        Month::June => "Juin",
        Month::July => "Juillet",
        Month::August => "Août",
        Month::September => "Septembre",
        Month::October => "Octobre",
        Month::November => "Novembre",
        Month::December => "Décembre",
    };
    Line::from(vec![
        " < ".fg(Color::Green),
        format!("{} {}", month, date.year()).fg(Color::White),
        " > ".fg(Color::Green),
    ])
    .bg(Color::Blue)
}

impl SelectedTab {
    /// Get the previous tab, if there is no previous tab return the current tab.
    fn previous(self) -> Self {
        let current_index: usize = self as usize;
        let previous_index = current_index.saturating_sub(1);
        Self::from_repr(previous_index).unwrap_or(self)
    }

    /// Get the next tab, if there is no next tab return the current tab.
    fn next(self) -> Self {
        let current_index = self as usize;
        let next_index = current_index.saturating_add(1);
        Self::from_repr(next_index).unwrap_or(self)
    }

    /// Return tab's name as a styled `Line`
    fn title(self) -> Line<'static> {
        format!(" {self} ").fg(Color::Black).bg(self.color()).into()
    }

    fn color(self) -> Color {
        match self {
            SelectedTab::Calendrier => Color::Yellow,
            SelectedTab::Bienvenue => Color::Cyan,
            SelectedTab::World => Color::Magenta,
            SelectedTab::Borders => Color::Green,
            SelectedTab::Wifi => Color::Red,
        }
    }
}

fn center(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

fn vcenter(area: Rect, vertical: Constraint) -> Rect {
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

pub const QUADRANT_OUTSIDE_TOP_FULL: border::Set = border::Set {
    top_right: "█",
    top_left: "█",
    bottom_left: "▙",
    bottom_right: "▟",
    vertical_left: "▌",
    vertical_right: "▐",
    horizontal_top: "█",
    horizontal_bottom: "▄",
};
