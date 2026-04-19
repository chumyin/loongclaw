pub trait TranslationProvider {
    fn translate(&self, key: &str) -> String;
}

pub struct I18nService {
    current_lang: crate::chat::pi_surface::command_palette::Language,
}

impl I18nService {
    pub fn new(lang: crate::chat::pi_surface::command_palette::Language) -> Self {
        Self { current_lang: lang }
    }

    pub fn set_lang(&mut self, lang: crate::chat::pi_surface::command_palette::Language) {
        self.current_lang = lang;
    }
}

impl TranslationProvider for I18nService {
    fn translate(&self, key: &str) -> String {
        match key {
            "cmd_language" => match self.current_lang {
                crate::chat::pi_surface::command_palette::Language::ZH_CN => "切换TUI显示语言".to_string(),
                crate::chat::pi_surface::command_palette::Language::ZH_TW => "切換TUI顯示語言".to_string(),
                _ => "Switch TUI language".to_string(),
            },
            _ => key.to_string(),
        }
    }
}
