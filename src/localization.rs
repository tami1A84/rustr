use fluent::{bundle::FluentBundle, FluentResource, FluentArgs};
use unic_langid::LanguageIdentifier;
use std::fs;
use intl_memoizer::IntlLangMemoizer;

pub struct LocalizationManager {
    bundle: FluentBundle<FluentResource, IntlLangMemoizer>,
    current_locale: LanguageIdentifier,
}

unsafe impl Send for LocalizationManager {}
unsafe impl Sync for LocalizationManager {}

impl LocalizationManager {
    pub fn new(locale_str: &str) -> Self {
        let current_locale: LanguageIdentifier = locale_str.parse().expect("Failed to parse locale");
        let resource = Self::load_resource(&current_locale);
        let mut bundle = FluentBundle::new(vec![current_locale.clone()]);
        bundle
            .add_resource(resource)
            .expect("Failed to add resource to bundle");

        LocalizationManager {
            bundle,
            current_locale,
        }
    }

    fn load_resource(locale: &LanguageIdentifier) -> FluentResource {
        let path = format!("locales/{}.ftl", locale);
        let ftl_string = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("Failed to read FTL file: {}", path));
        FluentResource::try_new(ftl_string).expect("Failed to parse FTL string")
    }

    pub fn get_message(&self, id: &str) -> String {
        self.get_message_with_args(id, None)
    }

    pub fn get_message_with_args(&self, id: &str, args: Option<&FluentArgs>) -> String {
        let msg = self.bundle.get_message(id).expect("Message not found");
        let mut errors = vec![];
        let pattern = msg.value().expect("Message has no value");
        let value = self.bundle.format_pattern(pattern, args, &mut errors);
        if !errors.is_empty() {
            eprintln!("Fluent format errors: {:?}", errors);
        }
        value.to_string()
    }
}
