//! Application logic for the Minitel app example.

use minitel::{
    prelude::*,
    ratatui::MinitelBackend,
    stum::videotex::{C0, FunctionKey, StringMessage, UserInput},
};
use ratatui::{
    prelude::*,
    style::Styled,
    symbols::border,
    widgets::{Block, Padding, Paragraph, Tabs, Widget, Wrap},
};
use std::io::{self, Cursor};
use strum::{Display, EnumIter, FromRepr, IntoEnumIterator};
use time::{Date, Duration, Month};

/// Application state
#[derive(Debug)]
pub struct App {
    selected_tab: SelectedTab,
    date: Date,
    demo_disjoint: bool,
    exit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            selected_tab: SelectedTab::default(),
            date: Date::from_calendar_date(2025, Month::January, 15).unwrap(),
            demo_disjoint: false,
            exit: false,
        }
    }
}

impl App {
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
}

impl Widget for &App {
    /// Draw the application to the ratatui buffer
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title_area = Rect::new(area.x, area.y, area.width, 1);
        let tabs_area = Rect::new(area.x, area.y + 1, area.width, 1);
        let instructions_height = 2.min(area.height.saturating_sub(2));
        let instructions_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(instructions_height),
            area.width,
            instructions_height,
        );
        let main_area = Rect::new(
            area.x,
            area.y + 2,
            area.width,
            area.height.saturating_sub(2 + instructions_height),
        );

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
        }

        self.draw_instructions(buf, instructions_area);
    }
}

impl App {
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
        Paragraph::new("Ratatui\n\ndans ton Minitel !")
            .style((Color::Blue, self.selected_tab.color()))
            .alignment(Alignment::Center)
            .render(main_area, buf);
    }

    fn draw_calendar(&self, buf: &mut Buffer, main_area: Rect) {
        let calendar_area = Rect::new(
            main_area.x + main_area.width.saturating_sub(23) / 2,
            main_area.y + 1,
            23.min(main_area.width),
            9.min(main_area.height.saturating_sub(1)),
        );
        let calendar_block = Block::bordered()
            .border_set(QUADRANT_OUTSIDE_TOP_FULL)
            .title(calendrier_title(self.date))
            .title_alignment(Alignment::Center)
            .style((Color::Blue, Color::White));
        let inner = calendar_block.inner(calendar_area);
        let weekdays_area = Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height));
        let days_area = Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        );
        calendar_block.render(calendar_area, buf);
        buf.set_style(days_area, Style::default().fg(Color::Blue).bg(Color::White));
        Paragraph::new(" Di Lu Ma Me Je Ve Sa ".fg(Color::Magenta).underlined())
            .render(weekdays_area, buf);
        Paragraph::new("  1  2  3  4  5  6  7\n  8  9 10 11 12 13 14\n 15 16 17 18 19 20 21\n 22 23 24 25 26 27 28")
            .style((Color::Blue, Color::White))
            .render(days_area, buf);
    }

    fn draw_world(&self, buf: &mut Buffer, main_area: Rect) {
        Paragraph::new("          WORLD\n\n       .-''''-.\n    .-'   _    '-.\n   /    _/ \\_    \\\n  ;   _/  __  \\_  ;\n  |  /   /  \\   \\ |\n  ;  \\_  \\__/  _/ ;\n   \\    '-.__.-'  /\n    '-.________.-'")
            .style((Color::Green, self.selected_tab.color()))
            .render(main_area, buf);
        buf.set_style(main_area, Style::default().crossed_out());
        // Force semi-graphic mode
        if self.demo_disjoint {
            buf.set_style(main_area, Style::default().underlined());
        }
    }

    fn draw_border_demo(&self, buf: &mut Buffer, main_area: Rect) {
        let inner = Rect::new(
            main_area.x + 1,
            main_area.y + 1,
            main_area.width.saturating_sub(2),
            main_area.height.saturating_sub(2),
        );
        let left_width = inner.width.saturating_sub(1) / 2;
        let right_width = inner.width.saturating_sub(1 + left_width);
        let h1 = Rect::new(inner.x, inner.y, left_width, inner.height);
        let h2 = Rect::new(inner.x + left_width + 1, inner.y, right_width, inner.height);
        let row_height = inner.height.saturating_sub(2) / 3;
        let last_row_height = inner.height.saturating_sub(2 + row_height * 2);
        let l11 = Rect::new(h1.x, h1.y, h1.width, row_height);
        let l12 = Rect::new(h1.x, h1.y + row_height + 1, h1.width, row_height);
        let l13 = Rect::new(h1.x, h1.y + (row_height + 1) * 2, h1.width, last_row_height);
        let l21 = Rect::new(h2.x, h2.y, h2.width, row_height);
        let l22 = Rect::new(h2.x, h2.y + row_height + 1, h2.width, row_height);
        let l23 = Rect::new(h2.x, h2.y + (row_height + 1) * 2, h2.width, last_row_height);

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
        }
    }
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
