//! Steam's favorite list (`AddFavoriteGame` flags 1 = favorite, 2 = history) without Steam:
//! the layout of Steam's `userdata/<id>/7/remote/serverbrowser_hist.vdf`, kept in
//! `platform/config`.

use std::net::{Ipv4Addr, SocketAddrV4};

use gs_gameui::Fs;
use gs_vgui::keyvalues::KeyValues;

const FILE: &str = "serverbrowser_hist.vdf";
pub const FAVORITE: u32 = 1;
pub const HISTORY: u32 = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub app: u32,
    /// Host order.
    pub ip: u32,
    pub conn_port: u16,
    pub query_port: u16,
    pub last_played: u32,
}

#[derive(Debug, Default)]
pub struct Store {
    favorites: Vec<Entry>,
    history: Vec<Entry>,
}

impl Store {
    pub fn load(fs: &Fs) -> Self {
        let mut store = Self::default();
        let Some(data) = fs.read_in(FILE, "CONFIG") else { return store };
        let mut kv = KeyValues::new("Filters");
        kv.load_from_buffer(&data);
        for (flags, key) in [(FAVORITE, "favorites"), (HISTORY, "history")] {
            let Some(list) = kv.find_key(key) else { continue };
            for e in &list.children {
                let Ok(addr) = e.get_string("address", "").parse::<SocketAddrV4>() else { continue };
                let entry = Entry { app: e.get_string("appid", "0").parse().unwrap_or(0), ip: u32::from(*addr.ip()), conn_port: addr.port(), query_port: addr.port(), last_played: e.get_string("LastPlayed", "0").parse().unwrap_or(0) };
                store.list_mut(flags).push(entry);
            }
        }
        store
    }

    fn save(&self, fs: &Fs) {
        let mut kv = KeyValues::new("Filters");
        for (key, list) in [("favorites", &self.favorites), ("history", &self.history)] {
            for (i, e) in list.iter().enumerate() {
                let address = SocketAddrV4::new(Ipv4Addr::from(e.ip), e.conn_port).to_string();
                let path = format!("{key}/{}", i + 1);
                kv.set_string(&format!("{path}/name"), &address);
                kv.set_string(&format!("{path}/address"), &address);
                kv.set_string(&format!("{path}/LastPlayed"), &e.last_played.to_string());
                kv.set_string(&format!("{path}/appid"), &e.app.to_string());
                kv.set_string(&format!("{path}/accountid"), "0");
            }
        }
        if !fs.write_in(FILE, "CONFIG", &kv.save_to_buffer()) {
            log::warn!("server browser: {FILE} not written");
        }
    }

    pub fn list(&self, flags: u32) -> &[Entry] {
        if flags & FAVORITE != 0 { &self.favorites } else { &self.history }
    }

    fn list_mut(&mut self, flags: u32) -> &mut Vec<Entry> {
        if flags & FAVORITE != 0 { &mut self.favorites } else { &mut self.history }
    }

    /// `GetFavoriteGameCount`: every entry, history included.
    pub fn count(&self) -> i32 {
        (self.favorites.len() + self.history.len()) as i32
    }

    /// `AddFavoriteGame`: a known address only takes the new time; the entry's index.
    pub fn add(&mut self, fs: &Fs, flags: u32, entry: Entry) -> i32 {
        let list = self.list_mut(flags);
        let i = match list.iter().position(|e| (e.ip, e.conn_port) == (entry.ip, entry.conn_port)) {
            Some(i) => {
                list[i].last_played = entry.last_played;
                i
            }
            None => {
                list.push(entry);
                list.len() - 1
            }
        };
        self.save(fs);
        i as i32
    }

    /// `RemoveFavoriteGame`, by address (the browser passes the app ID of the reply, which a
    /// typed address was stored without).
    pub fn remove(&mut self, fs: &Fs, flags: u32, ip: u32, conn_port: u16) -> bool {
        let list = self.list_mut(flags);
        let before = list.len();
        list.retain(|e| (e.ip, e.conn_port) != (ip, conn_port));
        let removed = list.len() != before;
        if removed {
            self.save(fs);
        }
        removed
    }

    /// When the address was last played (history), 0 when never.
    pub fn last_played(&self, ip: u32, conn_port: u16) -> u32 {
        self.favorites.iter().chain(&self.history).filter(|e| (e.ip, e.conn_port) == (ip, conn_port)).map(|e| e.last_played).max().unwrap_or(0)
    }
}
