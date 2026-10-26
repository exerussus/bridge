//! Узел моста: HTTP (соседи и локальный API), обмен с соседями, отправка файлов, общие папки, пространства, команды.

pub mod activity;
pub mod agent;
pub mod apps;
pub mod assistant;
pub mod avatar;
pub mod board;
pub mod builds;
pub mod board_api;
pub mod chanart;
pub mod chat;
pub mod client;
pub mod folders;
pub mod http_apps;
pub mod http_local;
pub mod http_peer;
pub mod iface;
pub mod link;
pub mod mailbox;
pub mod media;
pub mod mobile;
pub mod net;
pub mod node;
pub mod progress;
pub mod remote;
pub mod runner;
pub mod shares;
pub mod sound;
pub mod stickers;
pub mod tailscale;
pub mod tile;
pub mod turn;
pub mod update;
pub mod watch;

pub use node::{Node, StartOptions};
