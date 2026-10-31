//! The consent table (spec 9.7), fixed: what asks the person before a v1
//! app or a URL opens, and what never opens. Every path that opens
//! something for a v1 plugin asks [`ask`] which row it is; the texts of
//! the dialogs are here too, so the tests check both.
//!
//! Declining changes nothing: no chat, attachment, pin or open is made
//! before the person says yes.

/// What wants to open or attach (a row of 9.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A `buwiz://app/...` link from the operating system or a web page.
    OsLink,
    /// A tool result's `_meta["buwiz/open"]` with placement full.
    ToolResultFull,
    /// A tool result's `_meta["buwiz/open"]` with placement inline.
    ToolResultInline,
    /// The same from an approved first-party plugin (publisher
    /// `app.buwiz`): an answer card, mounted at once (9.8).
    FirstPartyAnswerCard,
    /// A click on an attachment or a pin (the person placed it).
    AttachmentOrPin,
    /// A command or token the person typed, resolving to `open`.
    UserTyped,
    /// A resolve `open` from an agent or a routine: as a tool result.
    AgentResolveOpen { inline: bool },
    /// An app's `buwiz.open({app})` (same plugin).
    AppOpenApp,
    /// An app's `buwiz.open({url})`.
    AppOpenUrl,
    /// An `https` URL in a resolve or tool result.
    PluginHttpsUrl,
    /// Another scheme in plugin output (`http`, `mailto`, `tel`, `file`,
    /// `buwiz://` other than `app`).
    PluginOtherScheme,
    /// An app's `buwiz.attach`.
    AppAttach,
    /// An app's `buwiz.pickFile`.
    AppPickFile,
}

/// What the person is shown before it goes ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Nothing: it goes ahead.
    Nothing,
    /// A dialog with the trigger's question; every time.
    Dialog,
    /// A card in the chat with Open; nothing opens until it is pressed.
    OpenCard,
    /// A card in the chat with Show; nothing mounts until it is pressed.
    ShowCard,
    /// The native file dialog is the consent.
    NativeDialog,
    /// Never opened.
    Refuse,
}

/// The 9.7 row of `t`.
pub fn ask(t: Trigger) -> Ask {
    match t {
        Trigger::OsLink | Trigger::AppOpenUrl | Trigger::PluginHttpsUrl | Trigger::AppAttach => Ask::Dialog,
        Trigger::ToolResultFull | Trigger::AgentResolveOpen { inline: false } => Ask::OpenCard,
        Trigger::ToolResultInline | Trigger::AgentResolveOpen { inline: true } => Ask::ShowCard,
        Trigger::AttachmentOrPin | Trigger::UserTyped | Trigger::AppOpenApp | Trigger::FirstPartyAnswerCard => {
            Ask::Nothing
        }
        Trigger::AppPickFile => Ask::NativeDialog,
        Trigger::PluginOtherScheme => Ask::Refuse,
    }
}

/// The row of a tool result's inline card (9.7, 9.8): a first-party
/// plugin's mounts at once, any other plugin's waits for Show.
pub fn inline_card(first_party: bool) -> Trigger {
    if first_party { Trigger::FirstPartyAnswerCard } else { Trigger::ToolResultInline }
}

/// The row a URL in plugin output falls under: `https` asks; a v1 app
/// link is a tool result's open (a card); anything else is refused.
pub fn plugin_url(url: &str) -> Trigger {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") && super::perms::https_origin(url.trim()).is_some() {
        Trigger::PluginHttpsUrl
    } else if lower.starts_with("buwiz://app/") {
        Trigger::ToolResultFull
    } else {
        Trigger::PluginOtherScheme
    }
}

/// "Open <app title> from <source>?" (an OS or web link).
pub fn open_app_question(app_title: &str, source: &str) -> String {
    format!("Open {app_title} from {source}?")
}

/// "Open <URL> in your browser?"
pub fn open_url_question(url: &str) -> String {
    format!("Open {url} in your browser?")
}

/// "Attach <title> to this chat?"
pub fn attach_question(title: &str) -> String {
    format!("Attach {title} to this chat?")
}

#[cfg(test)]
mod tests {
    use super::*;

    // CON-01: one test per 9.7 row: the consent is shown exactly where the
    // table says, and nowhere else. The bridge's own rows are also run
    // for real in bridge_tests (con_01_app_*), and the link and card rows
    // in links.rs and cards.rs.

    /// Every trigger, so each row's test also checks that only its own
    /// triggers give its answer.
    const ALL: [Trigger; 14] = [
        Trigger::OsLink,
        Trigger::ToolResultFull,
        Trigger::ToolResultInline,
        Trigger::FirstPartyAnswerCard,
        Trigger::AttachmentOrPin,
        Trigger::UserTyped,
        Trigger::AgentResolveOpen { inline: false },
        Trigger::AgentResolveOpen { inline: true },
        Trigger::AppOpenApp,
        Trigger::AppOpenUrl,
        Trigger::PluginHttpsUrl,
        Trigger::PluginOtherScheme,
        Trigger::AppAttach,
        Trigger::AppPickFile,
    ];

    fn with(a: Ask) -> Vec<Trigger> {
        ALL.into_iter().filter(|t| ask(*t) == a).collect()
    }

    #[test]
    fn con_01_os_link_asks_every_time() {
        assert_eq!(ask(Trigger::OsLink), Ask::Dialog);
        assert_eq!(open_app_question("BIR Forms", "another app"), "Open BIR Forms from another app?");
    }

    #[test]
    fn con_01_tool_result_open_is_a_card_with_open() {
        assert_eq!(ask(Trigger::ToolResultFull), Ask::OpenCard);
        assert_eq!(with(Ask::OpenCard), [Trigger::ToolResultFull, Trigger::AgentResolveOpen { inline: false }]);
    }

    #[test]
    fn con_01_tool_result_inline_is_a_card_with_show_unless_first_party() {
        // 9.8 replaces this row for approved first-party plugins: their
        // answer cards mount at once; every other plugin keeps Show
        assert_eq!(ask(inline_card(false)), Ask::ShowCard);
        assert_eq!(ask(inline_card(true)), Ask::Nothing);
        assert_eq!(with(Ask::ShowCard), [Trigger::ToolResultInline, Trigger::AgentResolveOpen { inline: true }]);
    }

    #[test]
    fn con_01_attachment_or_pin_click_asks_nothing() {
        assert_eq!(ask(Trigger::AttachmentOrPin), Ask::Nothing);
    }

    #[test]
    fn con_01_user_command_or_token_asks_nothing() {
        assert_eq!(ask(Trigger::UserTyped), Ask::Nothing);
    }

    #[test]
    fn con_01_agent_or_routine_resolve_open_is_a_tool_result() {
        assert_eq!(ask(Trigger::AgentResolveOpen { inline: false }), ask(Trigger::ToolResultFull));
        assert_eq!(ask(Trigger::AgentResolveOpen { inline: true }), ask(Trigger::ToolResultInline));
    }

    #[test]
    fn con_01_app_open_app_asks_nothing() {
        assert_eq!(ask(Trigger::AppOpenApp), Ask::Nothing);
        assert_eq!(
            with(Ask::Nothing),
            [Trigger::FirstPartyAnswerCard, Trigger::AttachmentOrPin, Trigger::UserTyped, Trigger::AppOpenApp]
        );
    }

    #[test]
    fn con_01_app_open_url_asks_every_time() {
        assert_eq!(ask(Trigger::AppOpenUrl), Ask::Dialog);
        assert_eq!(open_url_question("https://www.bir.gov.ph/x"), "Open https://www.bir.gov.ph/x in your browser?");
    }

    #[test]
    fn con_01_https_in_plugin_output_asks_every_time() {
        assert_eq!(plugin_url("https://www.bir.gov.ph/efps"), Trigger::PluginHttpsUrl);
        assert_eq!(ask(Trigger::PluginHttpsUrl), Ask::Dialog);
        assert_eq!(with(Ask::Dialog), [Trigger::OsLink, Trigger::AppOpenUrl, Trigger::PluginHttpsUrl, Trigger::AppAttach]);
    }

    #[test]
    fn con_01_other_schemes_from_plugin_output_are_refused() {
        for url in ["http://example.com", "mailto:a@b.c", "tel:+63", "file:///etc/hosts", "buwiz://chat/new?draft=x", "buwiz://view?path=/x", "javascript:x", "https://", "https://a@b.c/"] {
            assert_eq!(ask(plugin_url(url)), Ask::Refuse, "{url}");
        }
        // a v1 app link in plugin output is a tool result's open: a card
        assert_eq!(ask(plugin_url("buwiz://app/app.buwiz.ebirforms/forms")), Ask::OpenCard);
        assert_eq!(with(Ask::Refuse), [Trigger::PluginOtherScheme]);
    }

    #[test]
    fn con_01_app_attach_asks() {
        assert_eq!(ask(Trigger::AppAttach), Ask::Dialog);
        assert_eq!(attach_question("Dela Cruz Jane"), "Attach Dela Cruz Jane to this chat?");
    }

    #[test]
    fn con_01_app_pick_file_is_the_native_dialog() {
        assert_eq!(ask(Trigger::AppPickFile), Ask::NativeDialog);
        assert_eq!(with(Ask::NativeDialog), [Trigger::AppPickFile]);
    }
}
