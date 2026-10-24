//! mDNS advertise and browse (SPEC §9.3): service `_alstroemeria._tcp`, TXT `site`, `name`, `fp`, `schema`
//! and `pair=1` while in pairing mode. The remembered address in `local_peers` and manual entry are the
//! fallbacks when multicast doesn't work (the caller tries them).

use crate::{Error, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

const SERVICE: &str = "_alstroemeria._tcp.local.";

/// A device seen on the network.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovered {
    pub site_id: String,
    pub name: String,
    pub fp: String,
    pub addrs: Vec<String>,
    pub port: u16,
    pub schema: i64,
    /// The device is in pairing mode.
    pub pairing: bool,
}

impl Discovered {
    pub fn socket_addrs(&self) -> Vec<SocketAddr> {
        self.addrs.iter().filter_map(|a| a.parse::<IpAddr>().ok()).map(|ip| SocketAddr::new(ip, self.port)).collect()
    }
}

pub struct Advert {
    pub site_id: String,
    pub name: String,
    pub fp: String,
    pub port: u16,
    pub schema: i64,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    advert: Advert,
    found: Arc<Mutex<HashMap<String, Discovered>>>,
}

fn mdns<E: std::fmt::Display>(e: E) -> Error {
    Error::Protocol(format!("mdns: {e}"))
}

impl Discovery {
    /// Start advertising and browsing. `on_found` is called each time a device is resolved (new or refreshed);
    /// the sync triggers use it to sync "when a paired peer appears" (SPEC §9.4).
    pub fn start(advert: Advert, on_found: Arc<dyn Fn(&Discovered) + Send + Sync>) -> Result<Discovery> {
        let daemon = ServiceDaemon::new().map_err(mdns)?;
        let found: Arc<Mutex<HashMap<String, Discovered>>> = Arc::default();
        let d = Discovery { daemon, advert, found: found.clone() };
        d.register(false)?;
        let rx = d.daemon.browse(SERVICE).map_err(mdns)?;
        let own = d.advert.site_id.clone();
        std::thread::Builder::new().name("alst-mdns".into()).spawn(move || {
            while let Ok(ev) = rx.recv() {
                match ev {
                    ServiceEvent::ServiceResolved(info) => {
                        let Some(site) = info.get_property_val_str("site").map(String::from) else { continue };
                        if site == own {
                            continue;
                        }
                        let dev = Discovered {
                            name: info.get_property_val_str("name").unwrap_or("").to_string(),
                            fp: info.get_property_val_str("fp").unwrap_or("").to_string(),
                            addrs: info.get_addresses_v4().into_iter().map(|a| a.to_string()).collect(),
                            port: info.get_port(),
                            schema: info.get_property_val_str("schema").and_then(|s| s.parse().ok()).unwrap_or(0),
                            pairing: info.get_property_val_str("pair") == Some("1"),
                            site_id: site.clone(),
                        };
                        found.lock().unwrap_or_else(|e| e.into_inner()).insert(site, dev.clone());
                        on_found(&dev);
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        let site = fullname.split('.').next().unwrap_or("").to_string();
                        found.lock().unwrap_or_else(|e| e.into_inner()).remove(&site);
                    }
                    _ => {}
                }
            }
        })?;
        Ok(d)
    }

    fn register(&self, pairing: bool) -> Result<()> {
        let a = &self.advert;
        let addrs = crate::service::local_addrs();
        let host = format!("alst-{}.local.", &a.site_id[..a.site_id.len().min(12)]);
        let schema = a.schema.to_string();
        let mut props: Vec<(&str, &str)> = vec![("site", &a.site_id), ("name", &a.name), ("fp", &a.fp), ("schema", &schema)];
        if pairing {
            props.push(("pair", "1"));
        }
        let mut info = ServiceInfo::new(SERVICE, &a.site_id, &host, addrs.join(",").as_str(), a.port, &props[..]).map_err(mdns)?;
        if addrs.is_empty() {
            info = info.enable_addr_auto();
        }
        self.daemon.register(info).map_err(mdns)
    }

    /// Advertise `pair=1` while in pairing mode (re-registers the service with changed TXT records).
    pub fn set_pairing(&self, pairing: bool) -> Result<()> {
        self.register(pairing)
    }

    /// Update the advertised device name.
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        self.advert.name = name.to_string();
        self.register(false)
    }

    pub fn list(&self) -> Vec<Discovered> {
        let mut v: Vec<_> = self.found.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn addrs_for(&self, site_id: &str) -> Vec<SocketAddr> {
        self.found.lock().unwrap_or_else(|e| e.into_inner()).get(site_id).map(Discovered::socket_addrs).unwrap_or_default()
    }

    pub fn stop(&self) {
        let _ = self.daemon.shutdown();
    }
}
