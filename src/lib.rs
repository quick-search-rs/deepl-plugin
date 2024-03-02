use abi_stable::{
    export_root_module,
    prefix_type::PrefixTypeTrait,
    sabi_extern_fn,
    sabi_trait::prelude::TD_Opaque,
    std_types::{RBox, RStr, RString, RVec},
};
use quick_search_lib::{ColoredChar, Log, PluginId, SearchLib, SearchLib_Ref, SearchResult, Searchable, Searchable_TO};
use serde::{Deserialize, Serialize};

static NAME: &str = "DeepL-Translate";

#[export_root_module]
pub fn get_library() -> SearchLib_Ref {
    SearchLib { get_searchable }.leak_into_prefix()
}

#[sabi_extern_fn]
fn get_searchable(id: PluginId, logger: quick_search_lib::ScopedLogger) -> Searchable_TO<'static, RBox<()>> {
    let this = DeepL::new(id, logger);
    Searchable_TO::from_value(this, TD_Opaque)
}

struct DeepL {
    id: PluginId,
    client: reqwest::blocking::Client,
    config: quick_search_lib::Config,
    logger: quick_search_lib::ScopedLogger,
}

impl DeepL {
    fn new(id: PluginId, logger: quick_search_lib::ScopedLogger) -> Self {
        Self {
            id,
            logger,
            client: reqwest::blocking::Client::new(),
            config: default_config(),
        }
    }
}

impl Searchable for DeepL {
    fn search(&self, query: RString) -> RVec<SearchResult> {
        let mut res: Vec<SearchResult> = vec![];

        // let return_error_messages = self.config.get("Return Error messages").and_then(|entry| entry.as_bool()).unwrap_or(false);
        let api_key = self.config.get("DeepL Api Key").and_then(|entry| entry.as_string()).unwrap_or_default();

        if api_key.is_empty() {
            // if return_error_messages {
            //     res.push(SearchResult::new("No API key").set_context("No DeepL API key was provided"));
            // }
            self.logger.error("No API key was provided");
            return res.into();
        }
        // attempt to parse the query into one of:
        // <target_language_code>: <query>
        // <source_language_code> -> <target_language_code>: <query>
        // we will trim spaces so:
        // <source_language_code>-><target_language_code>:<query> is also valid

        // first, lets split on the first colon, if we get less than 2 parts, return the empty results early
        let mut parts = query.split(':');
        let query_codes = match parts.next() {
            Some(part) => part.trim().to_owned(),
            None => return res.into(),
        };

        if query_codes.is_empty() {
            return res.into();
        }

        // collect the rest of the parts into a string joined by colons (to fix the split)
        let rest = parts.map(|s| s.to_owned()).collect::<Vec<String>>().join(":");
        let rest = rest.trim().to_owned();

        if rest.is_empty() {
            // if return_error_messages {
            //     res.push(SearchResult::new("No query").set_context("No query was provided"));
            // }
            self.logger.trace("No query was provided");
            return res.into();
        }

        // now we can split the first part on the arrow, we should only get 1 or 2 parts, if we get 0 or more than 2, return the empty results early
        let mut parts = query_codes.split("->");
        let query = match (parts.next(), parts.next(), parts.next()) {
            (_, _, Some(_)) => {
                // if return_error_messages {
                //     res.push(SearchResult::new("Invalid query").set_context("Too many arrows"));
                // }
                self.logger.warn("Too many arrows in the query");
                return res.into();
            }
            (Some(source), Some(target), None) => {
                let source = source.trim().to_lowercase();
                let target = target.trim().to_lowercase();

                let source = match SourceLanguageCode::guess_from_str(&source) {
                    Some(code) => code,
                    None => {
                        // if return_error_messages {
                        //     res.push(SearchResult::new("Invalid query").set_context("Invalid source language code"));
                        // }
                        self.logger.warn("Invalid source language code");
                        return res.into();
                    }
                };

                let target = match TargetLanguageCode::guess_from_str(&target) {
                    Some(code) => code,
                    None => {
                        // if return_error_messages {
                        //     res.push(SearchResult::new("Invalid query").set_context("Invalid target language code"));
                        // }
                        self.logger.warn("Invalid target language code");
                        return res.into();
                    }
                };

                TranslateRequest {
                    text: vec![rest.clone()],
                    target_lang: target,
                    source_lang: Some(source),
                }
            }
            (Some(target), None, None) => {
                let target = target.trim().to_lowercase();

                let target = match TargetLanguageCode::guess_from_str(&target) {
                    Some(code) => code,
                    None => {
                        // if return_error_messages {
                        //     res.push(SearchResult::new("Invalid query").set_context("Invalid target language code"));
                        // }
                        self.logger.warn("Invalid target language code");
                        return res.into();
                    }
                };

                TranslateRequest {
                    text: vec![rest.clone()],
                    target_lang: target,
                    source_lang: None,
                }
            }
            _ => {
                // if return_error_messages {
                //     res.push(SearchResult::new("Invalid query").set_context("No target language code"));
                // }
                self.logger.warn("No target language code");
                return res.into();
            }
        };

        let use_free_tier = self.config.get("Use free tier").and_then(|entry| entry.as_bool()).unwrap_or(true);

        let response = match self
            .client
            .post(if use_free_tier {
                "https://api-free.deepl.com/v2/translate"
            } else {
                "https://api.deepl.com/v2/translate"
            })
            .header("Authorization", format!("DeepL-Auth-Key {}", api_key))
            .json(&query)
            .send()
        {
            Ok(response) => response,
            Err(e) => {
                // if return_error_messages {
                //     res.push(SearchResult::new("Request failed").set_context(&format!("Failed to send request: {}", e)));
                // }
                self.logger.error(&format!("Failed to send request: {}", e));
                return res.into();
            }
        };

        let response = match response.json::<TranslateResponse>() {
            Ok(response) => response,
            Err(e) => {
                // if return_error_messages {
                //     res.push(SearchResult::new("Response failed").set_context(&format!("Failed to parse response: {}", e)));
                // }
                self.logger.error(&format!("Failed to parse response: {}", e));
                return res.into();
            }
        };

        // by default, the clipboard will only contain the translated text

        // if true, the clipboard will contain the query, a newline, and the translated text
        let include_query_in_clipboard = self.config.get("Include query in clipboard").and_then(|entry| entry.as_bool()).unwrap_or(false);

        // if true, then format the query as <source_language_code>: <query> (if included) and format the translated text as <target_language_code>: <translated_text>
        let include_language_code_in_clipboard = self.config.get("Include language code in clipboard").and_then(|entry| entry.as_bool()).unwrap_or(false);

        for translation in response.translations {
            let query_str = if include_query_in_clipboard {
                if include_language_code_in_clipboard {
                    let source_lang = query.source_lang.unwrap_or(translation.detected_source_language);
                    format!("{}: {}\n", source_lang, rest)
                } else {
                    format!("{}\n", rest)
                }
            } else {
                "".to_owned()
            };

            let translated_str = if include_language_code_in_clipboard {
                format!("{}: {}", query.target_lang, translation.text)
            } else {
                translation.text.clone()
            };

            let clipboard_text = format!("{}{}", query_str, translated_str);

            res.push(SearchResult::new(&translation.text).set_extra_info(&clipboard_text));
        }

        res.into()
    }
    fn name(&self) -> RStr<'static> {
        NAME.into()
    }
    fn colored_name(&self) -> RVec<quick_search_lib::ColoredChar> {
        // can be dynamic although it's iffy how it might be used
        ColoredChar::from_string("DeepL", 0x2292A4FF)
    }
    fn execute(&self, result: &SearchResult) {
        let extra_info = result.extra_info();
        if !extra_info.is_empty() {
            if let Ok::<clipboard::ClipboardContext, Box<dyn std::error::Error>>(mut clipboard) = clipboard::ClipboardProvider::new() {
                if let Ok(()) = clipboard::ClipboardProvider::set_contents(&mut clipboard, extra_info.to_owned()) {
                    self.logger.trace(&format!("copied to clipboard: {}", extra_info));
                } else {
                    self.logger.error(&format!("failed to copy to clipboard: {}", extra_info));
                }
            } else {
                self.logger.error(&format!("failed to copy to clipboard: {}", extra_info));
            }
        }

        // finish up, above is a clipboard example
    }
    fn plugin_id(&self) -> PluginId {
        self.id.clone()
    }
    fn get_config_entries(&self) -> quick_search_lib::Config {
        default_config()
    }
    fn lazy_load_config(&mut self, config: quick_search_lib::Config) {
        self.config = config;
    }
}

fn default_config() -> quick_search_lib::Config {
    let mut config = quick_search_lib::Config::new();
    config.insert("DeepL Api Key".into(), quick_search_lib::EntryType::String { value: RString::new() });
    config.insert("Use free tier".into(), quick_search_lib::EntryType::Bool { value: true });
    // config.insert("Return Error messages".into(), quick_search_lib::EntryType::Bool { value: false });
    config.insert("Include query in clipboard".into(), quick_search_lib::EntryType::Bool { value: false });
    config.insert("Include language code in clipboard".into(), quick_search_lib::EntryType::Bool { value: false });
    config
}

// example request:
// POST /v2/translate HTTP/2
// Host: api-free.deepl.com
// Authorization: DeepL-Auth-Key [yourAuthKey]
// User-Agent: YourApp/1.2.3
// Content-Length: 45
// Content-Type: application/json

// {"text":["Hello, world!"],"target_lang":"DE"}

#[derive(Debug, Serialize)]
struct TranslateRequest {
    text: Vec<String>,
    target_lang: TargetLanguageCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_lang: Option<SourceLanguageCode>,
}

// example response:
// {
//   "translations": [
//     {
//       "detected_source_language": "EN",
//       "text": "Hallo, Welt!"
//     }
//   ]
// }

#[derive(Debug, Deserialize)]
struct TranslateResponse {
    translations: Vec<TranslatedText>,
}

#[derive(Debug, Deserialize)]
struct TranslatedText {
    detected_source_language: SourceLanguageCode,
    text: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum SourceLanguageCode {
    AR, // Arabic [1]
    BG, // Bulgarian
    CS, // Czech
    DA, // Danish
    DE, // German
    EL, // Greek
    EN, // English
    ES, // Spanish
    ET, // Estonian
    FI, // Finnish
    FR, // French
    HU, // Hungarian
    ID, // Indonesian
    IT, // Italian
    JA, // Japanese
    KO, // Korean
    LT, // Lithuanian
    LV, // Latvian
    NB, // Norwegian (Bokmål)
    NL, // Dutch
    PL, // Polish
    PT, // Portuguese (all Portuguese varieties mixed)
    RO, // Romanian
    RU, // Russian
    SK, // Slovak
    SL, // Slovenian
    SV, // Swedish
    TR, // Turkish
    UK, // Ukrainian
    ZH, // Chinese
}

impl SourceLanguageCode {
    fn guess_from_str(s: &str) -> Option<Self> {
        Some(match s {
            "ar" => SourceLanguageCode::AR,
            "arabic" => SourceLanguageCode::AR,
            "bg" => SourceLanguageCode::BG,
            "bulgarian" => SourceLanguageCode::BG,
            "cs" => SourceLanguageCode::CS,
            "czech" => SourceLanguageCode::CS,
            "da" => SourceLanguageCode::DA,
            "danish" => SourceLanguageCode::DA,
            "de" => SourceLanguageCode::DE,
            "german" => SourceLanguageCode::DE,
            "el" => SourceLanguageCode::EL,
            "greek" => SourceLanguageCode::EL,
            "en" => SourceLanguageCode::EN,
            "english" => SourceLanguageCode::EN,
            "es" => SourceLanguageCode::ES,
            "spanish" => SourceLanguageCode::ES,
            "et" => SourceLanguageCode::ET,
            "estonian" => SourceLanguageCode::ET,
            "fi" => SourceLanguageCode::FI,
            "finnish" => SourceLanguageCode::FI,
            "fr" => SourceLanguageCode::FR,
            "french" => SourceLanguageCode::FR,
            "hu" => SourceLanguageCode::HU,
            "hungarian" => SourceLanguageCode::HU,
            "id" => SourceLanguageCode::ID,
            "indonesian" => SourceLanguageCode::ID,
            "it" => SourceLanguageCode::IT,
            "italian" => SourceLanguageCode::IT,
            "jp" => SourceLanguageCode::JA,
            "ja" => SourceLanguageCode::JA,
            "japanese" => SourceLanguageCode::JA,
            "ko" => SourceLanguageCode::KO,
            "korean" => SourceLanguageCode::KO,
            "lt" => SourceLanguageCode::LT,
            "lithuanian" => SourceLanguageCode::LT,
            "lv" => SourceLanguageCode::LV,
            "latvian" => SourceLanguageCode::LV,
            "nb" => SourceLanguageCode::NB,
            "norwegian" => SourceLanguageCode::NB,
            "nl" => SourceLanguageCode::NL,
            "dutch" => SourceLanguageCode::NL,
            "pl" => SourceLanguageCode::PL,
            "polish" => SourceLanguageCode::PL,
            "pt" => SourceLanguageCode::PT,
            "portuguese" => SourceLanguageCode::PT,
            "ro" => SourceLanguageCode::RO,
            "romanian" => SourceLanguageCode::RO,
            "ru" => SourceLanguageCode::RU,
            "russian" => SourceLanguageCode::RU,
            "sk" => SourceLanguageCode::SK,
            "slovak" => SourceLanguageCode::SK,
            "sl" => SourceLanguageCode::SL,
            "slovenian" => SourceLanguageCode::SL,
            "sv" => SourceLanguageCode::SV,
            "swedish" => SourceLanguageCode::SV,
            "tr" => SourceLanguageCode::TR,
            "turkish" => SourceLanguageCode::TR,
            "uk" => SourceLanguageCode::UK,
            "ukrainian" => SourceLanguageCode::UK,
            "zh" => SourceLanguageCode::ZH,
            "chinese" => SourceLanguageCode::ZH,
            _ => return None,
        })
    }
}

impl std::fmt::Display for SourceLanguageCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceLanguageCode::AR => write!(f, "Arabic"),
            SourceLanguageCode::BG => write!(f, "Bulgarian"),
            SourceLanguageCode::CS => write!(f, "Czech"),
            SourceLanguageCode::DA => write!(f, "Danish"),
            SourceLanguageCode::DE => write!(f, "German"),
            SourceLanguageCode::EL => write!(f, "Greek"),
            SourceLanguageCode::EN => write!(f, "English"),
            SourceLanguageCode::ES => write!(f, "Spanish"),
            SourceLanguageCode::ET => write!(f, "Estonian"),
            SourceLanguageCode::FI => write!(f, "Finnish"),
            SourceLanguageCode::FR => write!(f, "French"),
            SourceLanguageCode::HU => write!(f, "Hungarian"),
            SourceLanguageCode::ID => write!(f, "Indonesian"),
            SourceLanguageCode::IT => write!(f, "Italian"),
            SourceLanguageCode::JA => write!(f, "Japanese"),
            SourceLanguageCode::KO => write!(f, "Korean"),
            SourceLanguageCode::LT => write!(f, "Lithuanian"),
            SourceLanguageCode::LV => write!(f, "Latvian"),
            SourceLanguageCode::NB => write!(f, "Norwegian (Bokmål)"),
            SourceLanguageCode::NL => write!(f, "Dutch"),
            SourceLanguageCode::PL => write!(f, "Polish"),
            SourceLanguageCode::PT => write!(f, "Portuguese"),
            SourceLanguageCode::RO => write!(f, "Romanian"),
            SourceLanguageCode::RU => write!(f, "Russian"),
            SourceLanguageCode::SK => write!(f, "Slovak"),
            SourceLanguageCode::SL => write!(f, "Slovenian"),
            SourceLanguageCode::SV => write!(f, "Swedish"),
            SourceLanguageCode::TR => write!(f, "Turkish"),
            SourceLanguageCode::UK => write!(f, "Ukrainian"),
            SourceLanguageCode::ZH => write!(f, "Chinese"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum TargetLanguageCode {
    AR, // Arabic [1]
    BG, // Bulgarian
    CS, // Czech
    DA, // Danish
    DE, // German
    EL, // Greek
    EN, // English (unspecified variant for backward compatibility; please select EN-GB or EN-US instead)
    #[serde(rename = "EN-GB")]
    EnGb, // English (British)
    #[serde(rename = "EN-US")]
    EnUs, // English (American)
    ES, // Spanish
    ET, // Estonian
    FI, // Finnish
    FR, // French
    HU, // Hungarian
    ID, // Indonesian
    IT, // Italian
    JA, // Japanese
    KO, // Korean
    LT, // Lithuanian
    LV, // Latvian
    NB, // Norwegian (Bokmål)
    NL, // Dutch
    PL, // Polish
    PT, // Portuguese (unspecified variant for backward compatibility; please select PT-BR or PT-PT instead)
    #[serde(rename = "PT-BR")]
    PtBr, // Portuguese (Brazilian)
    #[serde(rename = "PT-PT")]
    PtPt, // Portuguese (all Portuguese varieties excluding Brazilian Portuguese)
    RO, // Romanian
    RU, // Russian
    SK, // Slovak
    SL, // Slovenian
    SV, // Swedish
    TR, // Turkish
    UK, // Ukrainian
    ZH, // Chinese (simplified)
}

impl TargetLanguageCode {
    fn guess_from_str(s: &str) -> Option<Self> {
        Some(match s {
            "ar" => TargetLanguageCode::AR,
            "arabic" => TargetLanguageCode::AR,
            "bg" => TargetLanguageCode::BG,
            "bulgarian" => TargetLanguageCode::BG,
            "cs" => TargetLanguageCode::CS,
            "czech" => TargetLanguageCode::CS,
            "da" => TargetLanguageCode::DA,
            "danish" => TargetLanguageCode::DA,
            "de" => TargetLanguageCode::DE,
            "german" => TargetLanguageCode::DE,
            "el" => TargetLanguageCode::EL,
            "greek" => TargetLanguageCode::EL,
            "en" => TargetLanguageCode::EN,
            "english" => TargetLanguageCode::EN,
            "en-gb" => TargetLanguageCode::EnGb,
            "en-us" => TargetLanguageCode::EnUs,
            "es" => TargetLanguageCode::ES,
            "spanish" => TargetLanguageCode::ES,
            "et" => TargetLanguageCode::ET,
            "estonian" => TargetLanguageCode::ET,
            "fi" => TargetLanguageCode::FI,
            "finnish" => TargetLanguageCode::FI,
            "fr" => TargetLanguageCode::FR,
            "french" => TargetLanguageCode::FR,
            "hu" => TargetLanguageCode::HU,
            "hungarian" => TargetLanguageCode::HU,
            "id" => TargetLanguageCode::ID,
            "indonesian" => TargetLanguageCode::ID,
            "it" => TargetLanguageCode::IT,
            "italian" => TargetLanguageCode::IT,
            "jp" => TargetLanguageCode::JA,
            "ja" => TargetLanguageCode::JA,
            "japanese" => TargetLanguageCode::JA,
            "ko" => TargetLanguageCode::KO,
            "korean" => TargetLanguageCode::KO,
            "lt" => TargetLanguageCode::LT,
            "lithuanian" => TargetLanguageCode::LT,
            "lv" => TargetLanguageCode::LV,
            "latvian" => TargetLanguageCode::LV,
            "nb" => TargetLanguageCode::NB,
            "norwegian" => TargetLanguageCode::NB,
            "nl" => TargetLanguageCode::NL,
            "dutch" => TargetLanguageCode::NL,
            "pl" => TargetLanguageCode::PL,
            "polish" => TargetLanguageCode::PL,
            "pt" => TargetLanguageCode::PT,
            "portuguese" => TargetLanguageCode::PT,
            "pt-br" => TargetLanguageCode::PtBr,
            "pt-pt" => TargetLanguageCode::PtPt,
            "ro" => TargetLanguageCode::RO,
            "romanian" => TargetLanguageCode::RO,
            "ru" => TargetLanguageCode::RU,
            "russian" => TargetLanguageCode::RU,
            "sk" => TargetLanguageCode::SK,
            "slovak" => TargetLanguageCode::SK,
            "sl" => TargetLanguageCode::SL,
            "slovenian" => TargetLanguageCode::SL,
            "sv" => TargetLanguageCode::SV,
            "swedish" => TargetLanguageCode::SV,
            "tr" => TargetLanguageCode::TR,
            "turkish" => TargetLanguageCode::TR,
            "uk" => TargetLanguageCode::UK,
            "ukrainian" => TargetLanguageCode::UK,
            "zh" => TargetLanguageCode::ZH,
            "chinese" => TargetLanguageCode::ZH,
            _ => return None,
        })
    }
}

impl std::fmt::Display for TargetLanguageCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetLanguageCode::AR => write!(f, "Arabic"),
            TargetLanguageCode::BG => write!(f, "Bulgarian"),
            TargetLanguageCode::CS => write!(f, "Czech"),
            TargetLanguageCode::DA => write!(f, "Danish"),
            TargetLanguageCode::DE => write!(f, "German"),
            TargetLanguageCode::EL => write!(f, "Greek"),
            TargetLanguageCode::EN => write!(f, "English"),
            TargetLanguageCode::EnGb => write!(f, "English (British)"),
            TargetLanguageCode::EnUs => write!(f, "English (American)"),
            TargetLanguageCode::ES => write!(f, "Spanish"),
            TargetLanguageCode::ET => write!(f, "Estonian"),
            TargetLanguageCode::FI => write!(f, "Finnish"),
            TargetLanguageCode::FR => write!(f, "French"),
            TargetLanguageCode::HU => write!(f, "Hungarian"),
            TargetLanguageCode::ID => write!(f, "Indonesian"),
            TargetLanguageCode::IT => write!(f, "Italian"),
            TargetLanguageCode::JA => write!(f, "Japanese"),
            TargetLanguageCode::KO => write!(f, "Korean"),
            TargetLanguageCode::LT => write!(f, "Lithuanian"),
            TargetLanguageCode::LV => write!(f, "Latvian"),
            TargetLanguageCode::NB => write!(f, "Norwegian (Bokmål)"),
            TargetLanguageCode::NL => write!(f, "Dutch"),
            TargetLanguageCode::PL => write!(f, "Polish"),
            TargetLanguageCode::PT => write!(f, "Portuguese"),
            TargetLanguageCode::PtBr => write!(f, "Portuguese (Brazilian)"),
            TargetLanguageCode::PtPt => write!(f, "Portuguese (Other)"),
            TargetLanguageCode::RO => write!(f, "Romanian"),
            TargetLanguageCode::RU => write!(f, "Russian"),
            TargetLanguageCode::SK => write!(f, "Slovak"),
            TargetLanguageCode::SL => write!(f, "Slovenian"),
            TargetLanguageCode::SV => write!(f, "Swedish"),
            TargetLanguageCode::TR => write!(f, "Turkish"),
            TargetLanguageCode::UK => write!(f, "Ukrainian"),
            TargetLanguageCode::ZH => write!(f, "Chinese (simplified)"),
        }
    }
}
