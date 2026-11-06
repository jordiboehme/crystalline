//! What a new account can be refused with. The engine answers it, and the JSON API and
//! the daemon word it for who reads it.

/// Why `Engine::clear_name_for_new_account` refused. Each holds the sentence
/// for an admin; somebody else at the other end (a first sign-in) is told only
/// to ask one, so nothing about who held the name or what they left goes out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewAccountRefusal {
    /// The name holds a removed account's drafts.
    Drafts(String),
    /// A place the name's drafts can live in could not be read.
    Unread(String),
    /// A GitHub token saved under the name could not be deleted.
    Token(String),
}

impl NewAccountRefusal {
    /// The sentence for an admin.
    pub fn admin_sentence(&self) -> String {
        match self {
            NewAccountRefusal::Drafts(sentence) => sentence.clone(),
            NewAccountRefusal::Unread(what) => format!(
                "{what}, so the account was not made. The log of this instance has more."
            ),
            NewAccountRefusal::Token(_) => "an old GitHub token is still saved under this name and could not be deleted, so the account was not made. The log of this instance has more.".to_string(),
        }
    }

    /// What the log records.
    pub fn logged(&self) -> &str {
        match self {
            NewAccountRefusal::Drafts(text)
            | NewAccountRefusal::Unread(text)
            | NewAccountRefusal::Token(text) => text,
        }
    }
}

/// What a person who is not an admin reads when an account cannot be made under
/// a name: it names neither who held the name nor what they left.
pub const NEW_ACCOUNT_ASK_AN_ADMIN: &str =
    "An account cannot be made under this name yet. Ask an admin of this instance.";

/// What a person who is not an admin reads when a first sign-in meets the account
/// cap. The sentence that names the setting goes to the log.
pub const NEW_ACCOUNT_CAP_REACHED: &str =
    "This instance cannot take a new account right now. Ask an admin of this instance.";
