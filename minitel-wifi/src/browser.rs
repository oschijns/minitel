use embedded_svc::http::{Method, client::Client as HttpClient};
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection};
use minitel::{
    prelude::*,
    ratatui::MinitelBackend,
    stum::videotex::{C0, FunctionKey, UserInput},
};
use ratatui::{
    prelude::*,
    widgets::{Paragraph, Widget, Wrap},
};
use std::io::{self, Cursor};

const MAX_URL_BYTES: usize = 256;
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const DOCUMENT_TITLE: &str = " Minitel - navigateur texte ";

#[derive(Debug, Default)]
pub struct Browser {
    url: String,
    document: String,
    status: Option<(bool, String)>,
    scroll: u16,
    exit: bool,
}

impl Browser {
    pub async fn run<B: AsyncMinitelRead + AsyncMinitelWrite>(
        &mut self,
        minitel: &mut B,
    ) -> io::Result<()> {
        minitel.send(C0::FF).await?;
        let result = self.event_loop(minitel).await;
        minitel.send(C0::FF).await?;
        result
    }

    async fn event_loop<B: AsyncMinitelRead + AsyncMinitelReadWrite>(
        &mut self,
        minitel: &mut B,
    ) -> io::Result<()> {
        let backend = MinitelBackend::new(Cursor::new(Vec::new()));
        let mut terminal = Terminal::new(backend)?;

        while !self.exit {
            terminal.draw(|frame| frame.render_widget(&*self, frame.area()))?;
            let cursor = &mut terminal.backend_mut().stream;
            let buffer = cursor.get_mut();
            minitel.write(buffer).await?;
            buffer.clear();
            cursor.set_position(0);

            if let Ok(input) = minitel.read_s0_stroke().await {
                self.handle_input(input);
            }
        }
        Ok(())
    }

    fn handle_input(&mut self, input: UserInput) {
        match input {
            UserInput::FunctionKey(FunctionKey::Sommaire) => self.exit = true,
            UserInput::FunctionKey(FunctionKey::Envoi) => {
                self.status = Some(match fetch_document(&self.url) {
                    Ok(document) => {
                        self.document = document;
                        self.scroll = 0;
                        (true, "Page chargee.".to_string())
                    }
                    Err(error) => {
                        self.document.clear();
                        self.scroll = 0;
                        (false, error)
                    }
                });
            }
            UserInput::FunctionKey(FunctionKey::Suite) => {
                self.scroll = self.scroll.saturating_add(5);
            }
            UserInput::FunctionKey(FunctionKey::Retour) => {
                self.scroll = self.scroll.saturating_sub(5);
            }
            UserInput::FunctionKey(FunctionKey::Correction) => {
                self.url.pop();
                self.clear_document();
            }
            UserInput::FunctionKey(FunctionKey::Annulation) => {
                self.url.clear();
                self.clear_document();
            }
            UserInput::Char(character)
                if !character.is_control()
                    && self.url.len() + character.len_utf8() <= MAX_URL_BYTES =>
            {
                self.url.push(character);
                self.clear_document();
            }
            _ => {}
        }
    }

    fn clear_document(&mut self) {
        self.document.clear();
        self.scroll = 0;
        self.status = None;
    }
}

impl Widget for &Browser {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(DOCUMENT_TITLE)
            .style((Color::Yellow, Color::Black))
            .alignment(Alignment::Center)
            .render(
                Rect::new(area.x, area.y, area.width, 1.min(area.height)),
                buf,
            );

        Paragraph::new(format!("URL: {}", self.url))
            .style((Color::Black, Color::Yellow))
            .render(
                Rect::new(
                    area.x,
                    area.y.saturating_add(1),
                    area.width,
                    1.min(area.height.saturating_sub(1)),
                ),
                buf,
            );

        let content_area = Rect::new(
            area.x,
            area.y.saturating_add(3),
            area.width,
            area.height.saturating_sub(8),
        );
        Paragraph::new(self.document.as_str())
            .style((Color::White, Color::Blue))
            .wrap(Wrap { trim: true })
            .scroll((self.scroll, 0))
            .render(content_area, buf);

        if let Some((success, status)) = &self.status {
            Paragraph::new(status.as_str())
                .style((
                    if *success {
                        Color::Green
                    } else {
                        Color::Yellow
                    },
                    Color::Blue,
                ))
                .wrap(Wrap { trim: true })
                .render(
                    Rect::new(
                        area.x,
                        area.y.saturating_add(area.height.saturating_sub(5)),
                        area.width,
                        2.min(area.height.saturating_sub(3)),
                    ),
                    buf,
                );
        }

        Paragraph::new(concat![
            "Suite/Retour: defiler  Envoi: ouvrir URL\n",
            "Correction: effacer    Annulation: vider\n",
            "Sommaire: quitter"
        ])
        .style((Color::White, Color::Blue))
        .render(
            Rect::new(
                area.x,
                area.y.saturating_add(area.height.saturating_sub(3)),
                area.width,
                area.height.min(3),
            ),
            buf,
        );
    }
}

fn fetch_document(url: &str) -> Result<String, String> {
    let url = url.trim();
    validate_url(url)?;

    let config = HttpConfiguration {
        timeout: Some(std::time::Duration::from_secs(20)),
        buffer_size: Some(1024),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let connection =
        EspHttpConnection::new(&config).map_err(|error| format!("Client HTTP: {error}"))?;
    let mut client = HttpClient::wrap(connection);
    let headers = [("accept", "text/plain, text/html;q=0.9")];
    let mut response = client
        .request(Method::Get, url, &headers)
        .and_then(|request| request.submit())
        .map_err(|error| format!("Requete impossible: {error}"))?;

    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(format!("Le serveur a repondu avec le statut {status}."));
    }

    let content_type = response.header("Content-Type").unwrap_or("text/plain");
    if !content_type.starts_with("text/") && !content_type.contains("application/xhtml+xml") {
        return Err(format!("Type de document non texte: {content_type}"));
    }

    let mut bytes = Vec::with_capacity(MAX_RESPONSE_BYTES);
    let mut buffer = [0u8; 512];
    let mut truncated = false;
    loop {
        let remaining = MAX_RESPONSE_BYTES + 1 - bytes.len();
        let read_size = remaining.min(buffer.len());
        let count = response
            .read(&mut buffer[..read_size])
            .map_err(|error| format!("Lecture de la page impossible: {error}"))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_RESPONSE_BYTES {
            bytes.truncate(MAX_RESPONSE_BYTES);
            truncated = true;
            break;
        }
    }

    let mut document = decode_body(&bytes);
    if truncated {
        document.push_str("\n\n[Document tronque a 16 Ko]");
    }
    if document.trim().is_empty() {
        return Err("La page ne contient pas de texte affichable.".to_string());
    }
    Ok(document)
}

fn validate_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_URL_BYTES {
        return Err("URL trop longue (256 octets maximum).".to_string());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("L'URL doit commencer par http:// ou https://.".to_string());
    }
    let authority = url
        .split_once("://")
        .map(|(_, authority)| authority.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("Adresse serveur invalide.".to_string());
    }
    Ok(())
}

fn decode_body(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_http_urls_without_embedded_credentials() {
        assert!(validate_url("http://example.com/page").is_ok());
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("https://user@example.com").is_err());
    }

    #[test]
    fn keeps_response_markup_as_plain_text() {
        let response = b"<p>Hello &amp; welcome</p>";
        assert_eq!(decode_body(response), "<p>Hello &amp; welcome</p>");
    }
}
