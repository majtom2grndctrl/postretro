//! Owner tokens a `byPlayer` read or write may address.
//! See: context/lib/scripting.md §11 (interaction-pair scope) and §12 (Assist).

/// A player event's player (`on.player`).
pub const EVENT_PLAYER_OWNER_TOKEN: &str = "@player";
/// An impact policy's damager (`impact.source`).
pub const IMPACT_SOURCE_OWNER_TOKEN: &str = "@impact.source";
/// The helper side of an assist pair expression (`assist.helper`).
pub const ASSIST_HELPER_TOKEN: &str = "@assist.helper";
/// The target side of an assist pair expression (`assist.target`).
pub const ASSIST_TARGET_TOKEN: &str = "@assist.target";

/// Every owner token `byPlayer` recognizes, by the `__wire` spelling its SDK
/// handle carries. The engine-leaf installers in both runtimes (and their
/// build-time mirrors) are generated from this list, and a drift guard in
/// `postretro-sim` holds the hand-written SDK `storeRef` sites to it. Anything
/// else lowers to `@invalid`, which the binding scope rejects.
pub const BY_PLAYER_OWNER_TOKENS: [&str; 4] = [
    EVENT_PLAYER_OWNER_TOKEN,
    IMPACT_SOURCE_OWNER_TOKEN,
    ASSIST_HELPER_TOKEN,
    ASSIST_TARGET_TOKEN,
];

/// The owner-token set as a JavaScript array literal, for generated sources.
pub fn by_player_owner_tokens_js_array() -> String {
    let tokens: Vec<String> = BY_PLAYER_OWNER_TOKENS
        .iter()
        .map(|token| format!("\"{token}\""))
        .collect();
    format!("[{}]", tokens.join(", "))
}

/// The owner-token set as a frozen Luau membership table, for generated sources.
pub fn by_player_owner_tokens_luau_set() -> String {
    let entries: Vec<String> = BY_PLAYER_OWNER_TOKENS
        .iter()
        .map(|token| format!("[\"{token}\"] = true"))
        .collect();
    format!("table.freeze({{ {} }})", entries.join(", "))
}
