//! Kit de widgets OBS (`obs/*.html` + `src/config.js`) em Rust puro.
//!
//! Substitui integralmente `src/main/kit.ts` **sem executar JavaScript** (`node:vm`),
//! eliminando risco de RCE ao ler `src/config.js` via parser léxico estático de
//! literais de objeto e validando que o caminho pertence ao diretório autorizado.

use crate::protocol::{
    KitCameraRule, KitConfig, KitInfo, KitOverlay, KitParam, KitParamOption, KitRole,
    KitSocialLink,
};
use regex_lite::Regex;
use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use url::Url;

const KNOWN_KEYS: &[&str] = &[
    "canal",
    "nome",
    "jogo",
    "contagem",
    "agenda",
    "redes",
    "pix",
    "recados",
];

fn sumir_options() -> Vec<KitParamOption> {
    vec![
        KitParamOption {
            value: String::new(),
            label: "1 minuto".to_string(),
        },
        KitParamOption {
            value: "30".to_string(),
            label: "30 segundos".to_string(),
        },
        KitParamOption {
            value: "120".to_string(),
            label: "2 minutos".to_string(),
        },
        KitParamOption {
            value: "300".to_string(),
            label: "5 minutos".to_string(),
        },
        KitParamOption {
            value: "0".to_string(),
            label: "Nunca somem".to_string(),
        },
    ]
}

fn known_metadata(file: &str, url: String) -> Option<(usize, KitOverlay)> {
    let overlay = match file {
        "comecando.html" => (
            0,
            KitOverlay {
                file: file.to_string(),
                name: "Começando".to_string(),
                description: "Contagem regressiva; a estrela nasce e o céu clareia até zerar."
                    .to_string(),
                url,
                width: 1920,
                height: 1080,
                role: KitRole::Scene,
                restart_on_show: Some(true),
                camera: None,
                params: vec![
                    KitParam::Number {
                        key: "min".to_string(),
                        label: "Contagem".to_string(),
                        hint: None,
                        placeholder: "5".to_string(),
                        min: Some(1),
                        max: Some(120),
                        unit: Some("min".to_string()),
                    },
                    KitParam::Text {
                        key: "jogo".to_string(),
                        label: "Jogo de hoje".to_string(),
                        hint: Some(
                            "Vazio = segue a categoria da live (muda sozinho ao trocar o jogo)"
                                .to_string(),
                        ),
                        placeholder: "a categoria da live".to_string(),
                    },
                ],
            },
        ),
        "volto-ja.html" => (
            1,
            KitOverlay {
                file: file.to_string(),
                name: "Volto já".to_string(),
                description: "Cronômetro de quanto tempo você saiu e recados girando.".to_string(),
                url,
                width: 1920,
                height: 1080,
                role: KitRole::Scene,
                restart_on_show: Some(true),
                camera: None,
                params: vec![KitParam::Text {
                    key: "jogo".to_string(),
                    label: "Jogo/assunto".to_string(),
                    hint: Some("Vazio = segue a categoria da live".to_string()),
                    placeholder: "a categoria da live".to_string(),
                }],
            },
        ),
        "encerrando.html" => (
            2,
            KitOverlay {
                file: file.to_string(),
                name: "Encerrando".to_string(),
                description: "A estrela se põe devagar e mostra a próxima live.".to_string(),
                url,
                width: 1920,
                height: 1080,
                role: KitRole::Scene,
                restart_on_show: Some(true),
                camera: None,
                params: vec![
                    KitParam::Text {
                        key: "agenda".to_string(),
                        label: "Próxima live".to_string(),
                        hint: None,
                        placeholder: "a agenda dos textos do kit".to_string(),
                    },
                    KitParam::Text {
                        key: "msg".to_string(),
                        label: "Recado final".to_string(),
                        hint: None,
                        placeholder: "se ficou até aqui, você é estrela".to_string(),
                    },
                ],
            },
        ),
        "conversando.html" => (
            3,
            KitOverlay {
                file: file.to_string(),
                name: "Só conversando".to_string(),
                description:
                    "Moldura grande da câmera com o chat ao lado. A câmera vai por cima."
                        .to_string(),
                url,
                width: 1920,
                height: 1080,
                role: KitRole::Scene,
                restart_on_show: None,
                camera: Some(KitCameraRule::Param {
                    param: "cam".to_string(),
                    off: "0".to_string(),
                }),
                params: vec![
                    KitParam::Toggle {
                        key: "cam".to_string(),
                        label: "Moldura da câmera".to_string(),
                        hint: Some(
                            "Desligada, o espaço vira um cartaz com o seu nome".to_string(),
                        ),
                        on: String::new(),
                        off: "0".to_string(),
                        default_on: true,
                    },
                    KitParam::Select {
                        key: "sumir".to_string(),
                        label: "Mensagens do chat somem depois de".to_string(),
                        hint: None,
                        options: sumir_options(),
                    },
                    KitParam::Number {
                        key: "max".to_string(),
                        label: "Máximo de mensagens do chat".to_string(),
                        hint: None,
                        placeholder: "10".to_string(),
                        min: Some(1),
                        max: Some(40),
                        unit: None,
                    },
                ],
            },
        ),
        "gameplay.html" => (
            4,
            KitOverlay {
                file: file.to_string(),
                name: "Overlay de gameplay".to_string(),
                description:
                    "Transparente: moldura da câmera e barra de recados por cima do jogo."
                        .to_string(),
                url,
                width: 1920,
                height: 1080,
                role: KitRole::Overlay,
                restart_on_show: None,
                camera: Some(KitCameraRule::Param {
                    param: "cam".to_string(),
                    off: "none".to_string(),
                }),
                params: vec![
                    KitParam::Select {
                        key: "cam".to_string(),
                        label: "Moldura da câmera".to_string(),
                        hint: None,
                        options: vec![
                            KitParamOption {
                                value: String::new(),
                                label: "Embaixo à esquerda".to_string(),
                            },
                            KitParamOption {
                                value: "br".to_string(),
                                label: "Embaixo à direita".to_string(),
                            },
                            KitParamOption {
                                value: "tl".to_string(),
                                label: "Em cima à esquerda".to_string(),
                            },
                            KitParamOption {
                                value: "tr".to_string(),
                                label: "Em cima à direita".to_string(),
                            },
                            KitParamOption {
                                value: "none".to_string(),
                                label: "Sem moldura".to_string(),
                            },
                        ],
                    },
                    KitParam::Number {
                        key: "w".to_string(),
                        label: "Largura da câmera".to_string(),
                        hint: None,
                        placeholder: "480".to_string(),
                        min: Some(160),
                        max: Some(1280),
                        unit: Some("px".to_string()),
                    },
                    KitParam::Number {
                        key: "h".to_string(),
                        label: "Altura da câmera".to_string(),
                        hint: None,
                        placeholder: "270".to_string(),
                        min: Some(90),
                        max: Some(720),
                        unit: Some("px".to_string()),
                    },
                    KitParam::Toggle {
                        key: "chip".to_string(),
                        label: "Barra de recados".to_string(),
                        hint: None,
                        on: String::new(),
                        off: "0".to_string(),
                        default_on: true,
                    },
                ],
            },
        ),
        "webcam.html" => (
            5,
            KitOverlay {
                file: file.to_string(),
                name: "Moldura da câmera".to_string(),
                description: "Moldura avulsa no tamanho da câmera, para pôr por cima dela."
                    .to_string(),
                url,
                width: 480,
                height: 270,
                role: KitRole::Frame,
                restart_on_show: None,
                camera: Some(KitCameraRule::Hide("hide".to_string())),
                params: vec![KitParam::Toggle {
                    key: "nome".to_string(),
                    label: "Etiqueta com o nome".to_string(),
                    hint: None,
                    on: String::new(),
                    off: "0".to_string(),
                    default_on: true,
                }],
            },
        ),
        "chat.html" => (
            6,
            KitOverlay {
                file: file.to_string(),
                name: "Chat (multichat)".to_string(),
                description:
                    "Twitch, YouTube e Kick juntos no estilo do canal. As mensagens antigas somem sozinhas."
                        .to_string(),
                url,
                width: 520,
                height: 900,
                role: KitRole::Chat,
                restart_on_show: None,
                camera: None,
                params: vec![
                    KitParam::Text {
                        key: "canal".to_string(),
                        label: "Canal da Twitch".to_string(),
                        hint: Some(
                            "YouTube e Kick: no chat do app, em Opções do chat".to_string(),
                        ),
                        placeholder: "o canal dos textos do kit".to_string(),
                    },
                    KitParam::Select {
                        key: "sumir".to_string(),
                        label: "Mensagem some depois de".to_string(),
                        hint: None,
                        options: sumir_options(),
                    },
                    KitParam::Number {
                        key: "max".to_string(),
                        label: "Máximo de mensagens na tela".to_string(),
                        hint: None,
                        placeholder: "8".to_string(),
                        min: Some(1),
                        max: Some(40),
                        unit: None,
                    },
                    KitParam::Toggle {
                        key: "fundo".to_string(),
                        label: "Fundo escuro sólido".to_string(),
                        hint: None,
                        on: "1".to_string(),
                        off: String::new(),
                        default_on: false,
                    },
                ],
            },
        ),
        _ => return None,
    };
    Some(overlay)
}

fn validate_kit_dir(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Caminho do kit vazio.".to_string());
    }
    if trimmed.starts_with("\\\\") || trimmed.starts_with("//") {
        return Err("Caminhos de rede UNC não são permitidos para kits.".to_string());
    }
    let path = Path::new(trimmed);
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return Err("Path traversal ('..') não é permitido.".to_string());
        }
        if let Component::Prefix(prefix) = comp {
            if matches!(
                prefix.kind(),
                std::path::Prefix::UNC(..) | std::path::Prefix::VerbatimUNC(..)
            ) {
                return Err("Caminhos de rede UNC não são permitidos para kits.".to_string());
            }
        }
    }
    fs::canonicalize(path).map_err(|e| format!("Diretório do kit inválido: {e}"))
}

fn resolve_kit_config_path(raw_dir: &str) -> Result<PathBuf, String> {
    let root = validate_kit_dir(raw_dir)?;
    let cfg = root.join("src").join("config.js");
    if !cfg.exists() {
        return Err("O kit não tem src/config.js.".to_string());
    }
    let canon_cfg = fs::canonicalize(&cfg).map_err(|e| format!("src/config.js inacessível: {e}"))?;
    if !canon_cfg.starts_with(&root) {
        return Err("src/config.js aponta para fora da pasta do kit.".to_string());
    }
    Ok(canon_cfg)
}

fn title_of(html: &str, file: &str) -> String {
    if let Ok(re) = Regex::new(r"(?is)<title>\s*([^<]+?)\s*</title>") {
        if let Some(caps) = re.captures(html) {
            if let Some(m) = caps.get(1) {
                let t = m.as_str().trim();
                if !t.is_empty() {
                    return t.to_string();
                }
            }
        }
    }
    Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file)
        .to_string()
}

pub fn load_kit(path: Option<&str>) -> Option<KitInfo> {
    let raw = path?;
    let root = validate_kit_dir(raw).ok()?;
    let obs_dir = root.join("obs");
    if !obs_dir.is_dir() {
        return None;
    }

    let entries = fs::read_dir(&obs_dir).ok()?;
    let mut widgets_with_order: Vec<(usize, KitOverlay)> = Vec::new();

    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Some(file_name) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if !file_name.to_lowercase().ends_with(".html") {
            continue;
        }
        let Ok(url) = Url::from_file_path(&p) else {
            continue;
        };
        let url_str = url.to_string();
        if let Some((ord, overlay)) = known_metadata(&file_name, url_str.clone()) {
            widgets_with_order.push((ord, overlay));
        } else {
            let html = fs::read_to_string(&p).unwrap_or_default();
            widgets_with_order.push((
                99,
                KitOverlay {
                    file: file_name.clone(),
                    name: title_of(&html, &file_name),
                    description: "Página HTML do kit.".to_string(),
                    url: url_str,
                    width: 1920,
                    height: 1080,
                    role: KitRole::Overlay,
                    params: Vec::new(),
                    restart_on_show: None,
                    camera: None,
                },
            ));
        }
    }

    widgets_with_order.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));

    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("kit")
        .to_string();

    Some(KitInfo {
        path: raw.to_string(),
        name,
        widgets: widgets_with_order.into_iter().map(|(_, w)| w).collect(),
        has_stream_elements: root.join("streamelements").join("widget.js").is_file(),
    })
}

// ------------------------------------------------------------------ Parser estático seguro de config.js

struct JsObjectParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> JsObjectParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
        }
    }

    fn rest_str(&self) -> Result<&'a str, String> {
        std::str::from_utf8(&self.src[self.pos..]).map_err(|e| format!("UTF-8 inválido: {e}"))
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws_and_comments(&mut self) -> Result<(), String> {
        loop {
            while let Some(b) = self.peek() {
                if b.is_ascii_whitespace() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.src[self.pos..].starts_with(b"//") {
                self.pos += 2;
                while let Some(b) = self.peek() {
                    self.pos += 1;
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if self.src[self.pos..].starts_with(b"/*") {
                self.pos += 2;
                let mut closed = false;
                while self.pos + 1 < self.src.len() {
                    if self.src[self.pos] == b'*' && self.src[self.pos + 1] == b'/' {
                        self.pos += 2;
                        closed = true;
                        break;
                    }
                    self.pos += 1;
                }
                if !closed {
                    return Err("Comentário de bloco não fechado.".to_string());
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn parse_ident(&mut self) -> Result<String, String> {
        let start = self.pos;
        match self.peek() {
            Some(b) if b.is_ascii_alphabetic() || b == b'_' || b == b'$' => self.pos += 1,
            _ => return Err("Identificador inválido.".to_string()),
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                self.pos += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .map(str::to_string)
            .map_err(|e| e.to_string())
    }

    fn parse_string(&mut self, quote: u8) -> Result<String, String> {
        if self.peek() != Some(quote) {
            return Err("Esperado início de string.".to_string());
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == quote {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                let Some(esc) = self.peek() else {
                    return Err("Escape incompleto na string.".to_string());
                };
                self.pos += 1;
                match esc {
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'\\' => out.push('\\'),
                    b'\'' => out.push('\''),
                    b'"' => out.push('"'),
                    b'u' => {
                        if self.pos + 4 > self.src.len() {
                            return Err("Escape \\uXXXX inválido.".to_string());
                        }
                        let hex = std::str::from_utf8(&self.src[self.pos..self.pos + 4])
                            .map_err(|_| "Escape \\uXXXX inválido".to_string())?;
                        let code = u16::from_str_radix(hex, 16)
                            .map_err(|_| "Escape \\uXXXX inválido".to_string())?;
                        self.pos += 4;
                        if let Some(ch) = char::from_u32(code as u32) {
                            out.push(ch);
                        }
                    }
                    other => out.push(other as char),
                }
            } else {
                let s = self.rest_str()?;
                let ch = s.chars().next().ok_or("String inesperadamente vazia")?;
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        Err("String não terminada.".to_string())
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let mut has_digit = false;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                has_digit = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if !has_digit {
            return Err("Número inválido.".to_string());
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        let raw = std::str::from_utf8(&self.src[start..self.pos]).map_err(|e| e.to_string())?;
        if is_float {
            let f: f64 = raw.parse().map_err(|_| "Float inválido".to_string())?;
            Number::from_f64(f)
                .map(Value::Number)
                .ok_or_else(|| "Float não finito".to_string())
        } else {
            let n: i64 = raw.parse().map_err(|_| "Inteiro inválido".to_string())?;
            Ok(Value::Number(Number::from(n)))
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 16 {
            return Err("Profundidade máxima excedida em config.js.".to_string());
        }
        self.skip_ws_and_comments()?;
        match self.peek() {
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'"') => self.parse_string(b'"').map(Value::String),
            Some(b'\'') => self.parse_string(b'\'').map(Value::String),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(b) if b.is_ascii_alphabetic() => {
                let word = self.parse_ident()?;
                match word.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "null" => Ok(Value::Null),
                    other => Err(format!(
                        "Expressão não estática rejeitada em config.js: '{other}'"
                    )),
                }
            }
            _ => Err("Token inesperado ao ler valor em config.js.".to_string()),
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, String> {
        if self.peek() != Some(b'[') {
            return Err("Esperado '['.".to_string());
        }
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_ws_and_comments()?;
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(Value::Array(items));
            }
            items.push(self.parse_value(depth)?);
            self.skip_ws_and_comments()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err("Esperado ',' ou ']' em array.".to_string()),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, String> {
        if self.peek() != Some(b'{') {
            return Err("Esperado '{'.".to_string());
        }
        self.pos += 1;
        let mut map = Map::new();
        loop {
            self.skip_ws_and_comments()?;
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(Value::Object(map));
            }
            let key = match self.peek() {
                Some(b'"') => self.parse_string(b'"')?,
                Some(b'\'') => self.parse_string(b'\'')?,
                _ => self.parse_ident()?,
            };
            self.skip_ws_and_comments()?;
            if self.peek() != Some(b':') {
                return Err(format!("Esperado ':' após chave '{key}'."));
            }
            self.pos += 1;
            let val = self.parse_value(depth)?;
            map.insert(key, val);

            self.skip_ws_and_comments()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err("Esperado ',' ou '}' em objeto.".to_string()),
            }
        }
    }
}

pub fn parse_kit_config_str(src: &str) -> Result<(String, KitConfig), String> {
    let stripped = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut parser = JsObjectParser::new(stripped);
    parser.skip_ws_and_comments()?;

    // Aceita `window.KAYO = { ... };` (ou `var/const/let KAYO = { ... }` / `KAYO = { ... }`)
    let mut global_name = parser.parse_ident()?;
    if global_name == "window" {
        parser.skip_ws_and_comments()?;
        if parser.peek() != Some(b'.') {
            return Err("Esperado '.' após 'window'.".to_string());
        }
        parser.pos += 1;
        parser.skip_ws_and_comments()?;
        global_name = parser.parse_ident()?;
    } else if matches!(global_name.as_str(), "var" | "let" | "const") {
        parser.skip_ws_and_comments()?;
        global_name = parser.parse_ident()?;
    }

    parser.skip_ws_and_comments()?;
    if parser.peek() != Some(b'=') {
        return Err("Esperado '=' após identificador global.".to_string());
    }
    parser.pos += 1;

    let obj = parser.parse_value(0)?;
    parser.skip_ws_and_comments()?;
    if parser.peek() == Some(b';') {
        parser.pos += 1;
    }
    parser.skip_ws_and_comments()?;
    if parser.pos < parser.src.len() {
        return Err("Código extra não permitido após o objeto em config.js.".to_string());
    }

    let Value::Object(map) = obj else {
        return Err("config.js deve atribuir um objeto literal.".to_string());
    };

    let str_field = |k: &str| -> String {
        map.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let redes = map
        .get("redes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    Some(KitSocialLink {
                        rede: obj
                            .get("rede")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        handle: obj
                            .get("handle")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let recados = map
        .get("recados")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    let mut extra = HashMap::new();
    for (k, v) in &map {
        if !KNOWN_KEYS.contains(&k.as_str()) {
            extra.insert(k.clone(), v.clone());
        }
    }

    let contagem = map
        .get("contagem")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .clamp(1, 180) as u32;

    Ok((
        global_name,
        KitConfig {
            canal: str_field("canal"),
            nome: str_field("nome"),
            jogo: str_field("jogo"),
            contagem,
            agenda: str_field("agenda"),
            pix: str_field("pix"),
            redes,
            recados,
            extra,
        },
    ))
}

pub fn read_kit_config(path: &str) -> Option<KitConfig> {
    let cfg_path = resolve_kit_config_path(path).ok()?;
    let content = fs::read_to_string(&cfg_path).ok()?;
    parse_kit_config_str(&content).ok().map(|(_, cfg)| cfg)
}

fn is_valid_js_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

pub fn write_kit_config(path: &str, c: &KitConfig) -> Result<(), String> {
    let cfg_path = resolve_kit_config_path(path)?;
    let existing = fs::read_to_string(&cfg_path).unwrap_or_default();
    let global_name = parse_kit_config_str(&existing)
        .map(|(name, _)| name)
        .ok()
        .filter(|n| is_valid_js_ident(n))
        .unwrap_or_else(|| "KAYO".to_string());

    let q = |v: &str| serde_json::to_string(v).unwrap_or_else(|_| "\"\"".to_string());

    let redes = c
        .redes
        .iter()
        .filter(|r| !r.rede.trim().is_empty() || !r.handle.trim().is_empty())
        .map(|r| {
            format!(
                "    {{ rede: {}, handle: {} }},",
                q(r.rede.trim()),
                q(r.handle.trim())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let recados = c
        .recados
        .iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| format!("    {},", q(r.trim())))
        .collect::<Vec<_>>()
        .join("\n");

    let mut extra_keys: Vec<&String> = c.extra.keys().collect();
    extra_keys.sort();
    let mut extra_block = String::new();
    for k in extra_keys {
        let key_repr = if is_valid_js_ident(k) {
            k.clone()
        } else {
            q(k)
        };
        let val_pretty = serde_json::to_string_pretty(&c.extra[k])
            .unwrap_or_else(|_| "null".to_string())
            .replace('\n', "\n  ");
        extra_block.push_str(&format!("\n  {key_repr}: {val_pretty},\n"));
    }

    let contagem = c.contagem.clamp(1, 180);
    let rendered = format!(
        "// Edite aqui (ou no PrismCast: Widgets → Textos do kit) e todas as telas se atualizam sozinhas.\n\
window.{global_name} = {{\n\
  canal: {},\n\
  nome: {},\n\
\n\
  // Jogo/assunto padrão das telas \"começando\" e \"volto já\" (dá pra trocar pela URL: ?jogo=Dark%20Souls)\n\
  jogo: {},\n\
\n\
  // Minutos da contagem regressiva da tela \"começando\" (ou pela URL: ?min=10)\n\
  contagem: {contagem},\n\
\n\
  // Texto da próxima live na tela de encerramento e na tela offline\n\
  agenda: {},\n\
\n\
  redes: [\n\
{redes}\n\
  ],\n\
\n\
  pix: {},\n\
\n\
  // Frases que giram na tela \"volto já\" e na barra do overlay de gameplay\n\
  recados: [\n\
{recados}\n\
  ],\n\
{extra_block}}};\n",
        q(&c.canal),
        q(&c.nome),
        q(&c.jogo),
        q(&c.agenda),
        q(&c.pix),
    );

    let backup = cfg_path.with_file_name("config.js.original");
    if !backup.exists() {
        let _ = fs::copy(&cfg_path, &backup);
    }

    let tmp = cfg_path.with_file_name("config.js.tmp");
    let mut file = File::create(&tmp).map_err(|e| format!("Falha ao gravar tmp: {e}"))?;
    file.write_all(rendered.as_bytes())
        .map_err(|e| format!("Falha ao escrever tmp: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("Falha ao sincronizar tmp: {e}"))?;
    drop(file);
    fs::rename(&tmp, &cfg_path).map_err(|e| format!("Falha ao substituir config.js: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_real_config_js_with_comments_and_trailing_commas() {
        let src = r##"
        // Comentário inicial
        window.KAYO = {
          canal: 'kayoinreality',
          'nome': "Kayo",
          /* Bloco
             de comentário */
          jogo: "Elden Ring",
          contagem: 10,
          agenda: "Seg/Qua/Sex 20h",
          redes: [
            { rede: 'Twitch', handle: '@kayoinreality', },
            { "rede": "YouTube", "handle": "@kayo" },
          ],
          pix: 'chave@pix.br',
          recados: [
            'Bem-vindos!',
            "Deixa o sub!",
          ],
          tema: { cor: "#8b5cf6", ativo: true, },
        };
        "##;

        let (ident, cfg) = parse_kit_config_str(src).expect("should parse valid config.js");
        assert_eq!(ident, "KAYO");
        assert_eq!(cfg.canal, "kayoinreality");
        assert_eq!(cfg.nome, "Kayo");
        assert_eq!(cfg.contagem, 10);
        assert_eq!(cfg.redes.len(), 2);
        assert_eq!(cfg.recados.len(), 2);
        assert!(cfg.extra.contains_key("tema"));
    }

    #[test]
    fn test_rejects_malicious_js_and_path_traversal() {
        let payloads = [
            "window.KAYO = { canal: require('child_process').execSync('calc') };",
            "(() => { process.exit(1); })();",
            "window.KAYO = { canal: 'ok' }; fetch('https://evil.test');",
            "window.KAYO = { canal: process.env.SECRET };",
        ];
        for p in payloads {
            assert!(
                parse_kit_config_str(p).is_err(),
                "payload should be rejected: {p}"
            );
        }

        assert!(validate_kit_dir(r"\\evil-server\share\kit").is_err());
        assert!(validate_kit_dir(r"C:\kits\..\Windows").is_err());
    }

    #[test]
    fn test_load_and_write_roundtrip() {
        let dir = std::env::temp_dir().join(format!("prismcast-kit-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("obs")).unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("obs").join("comecando.html"), "<html></html>").unwrap();
        fs::write(
            dir.join("obs").join("custom.html"),
            "<html><head><title>Meu Overlay</title></head></html>",
        )
        .unwrap();
        fs::write(
            dir.join("src").join("config.js"),
            "window.MYKIT = { canal: 'abc', contagem: 5, redes: [], recados: [] };",
        )
        .unwrap();

        let dir_str = dir.to_string_lossy().to_string();
        let info = load_kit(Some(&dir_str)).expect("kit loaded");
        assert_eq!(info.widgets.len(), 2);
        assert_eq!(info.widgets[0].file, "comecando.html");
        assert_eq!(info.widgets[1].name, "Meu Overlay");

        let mut cfg = read_kit_config(&dir_str).expect("config read");
        cfg.jogo = "Hollow Knight".to_string();
        cfg.redes.push(KitSocialLink {
            rede: "Kick".to_string(),
            handle: "kayo".to_string(),
        });
        write_kit_config(&dir_str, &cfg).expect("config written");

        assert!(dir.join("src").join("config.js.original").exists());
        let reread = read_kit_config(&dir_str).expect("config reread");
        assert_eq!(reread.jogo, "Hollow Knight");
        assert_eq!(reread.redes.len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }
}
