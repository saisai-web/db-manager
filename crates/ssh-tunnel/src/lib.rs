//! SSH のローカルポート転送。
//!
//! - 待ち受けるのは `127.0.0.1` の空きポートだけ(他のマシンからは入れない)
//! - ホスト鍵は、登録済みの指紋と一致したときだけ信頼する。未登録・不一致は、認証に進む前に止める
//!   (パスワードを、確かめていないサーバーに送らない)

mod config;
mod error;
mod tunnel;

pub use config::{SshAuth, TunnelConfig};
pub use error::TunnelError;
pub use tunnel::Tunnel;
