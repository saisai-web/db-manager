use driver_api::LexicalFeatures;
use sqlscan::Rules;

/// ドライバ(Dialect)が示す字句の機能を、文分割の規則にする。DB の名前は見ない。
pub(crate) fn scan_rules(f: LexicalFeatures) -> Rules {
    Rules {
        dollar_quote: f.dollar_quote,
        e_strings: f.e_strings,
        nested_block_comments: f.nested_block_comments,
        backtick_ident: f.backtick_ident,
        hash_comment: f.hash_comment,
        bracket_ident: f.bracket_ident,
        backslash_escapes: f.backslash_escapes,
        q_quote: f.q_quote,
        plsql: f.plsql,
    }
}
