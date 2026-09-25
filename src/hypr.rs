use crate::json::{self, Json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub width: i32,
    pub height: i32,
    pub refresh_rate: f64,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: i32,
    pub disabled: bool,
    pub available_modes: Vec<String>,
}

pub fn parse_mode(s: &str) -> Option<(i32, i32, f64)> {
    let (res, hz) = s.split_once('@')?;
    let (w, h) = res.split_once('x')?;
    let hz = hz.trim_end_matches("Hz").parse().ok()?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?, hz))
}

pub fn fmt_f(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".into()
    } else {
        s.to_string()
    }
}

impl Monitor {
    pub fn logical_size(&self) -> (f64, f64) {
        let (w, h) = if self.transform % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        ((w as f64 / s).round(), (h as f64 / s).round())
    }

    pub fn mode_string(&self) -> String {
        format!(
            "{}x{}@{}",
            self.width,
            self.height,
            fmt_f(self.refresh_rate)
        )
    }

    pub fn to_lua(&self) -> String {
        if self.disabled {
            return format!(
                "hl.monitor({{ output = \"{}\", disabled = true }})",
                self.name
            );
        }
        format!(
            "hl.monitor({{ output = \"{}\", mode = \"{}\", position = \"{}x{}\", scale = {}, transform = {}, disabled = false }})",
            self.name,
            self.mode_string(),
            self.x,
            self.y,
            fmt_f(self.scale),
            self.transform
        )
    }

    pub fn to_keyword(&self) -> String {
        if self.disabled {
            return format!("{},disable", self.name);
        }
        let mut s = format!(
            "{},{},{}x{},{}",
            self.name,
            self.mode_string(),
            self.x,
            self.y,
            fmt_f(self.scale)
        );
        if self.transform != 0 {
            s.push_str(&format!(",transform,{}", self.transform));
        }
        s
    }
}

fn request(req: &str) -> Result<String, String> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| "HYPRLAND_INSTANCE_SIGNATURE not set; is Hyprland running?")?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let mut s = [format!("{runtime}/hypr/{sig}"), format!("/tmp/hypr/{sig}")]
        .iter()
        .find_map(|d| UnixStream::connect(format!("{d}/.socket.sock")).ok())
        .ok_or("could not connect to the Hyprland socket")?;
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn monitor(j: &Json) -> Monitor {
    Monitor {
        id: j.num("id") as i64,
        name: j.str("name"),
        description: j.str("description"),
        width: j.num("width") as i32,
        height: j.num("height") as i32,
        refresh_rate: j.num("refreshRate"),
        x: j.num("x") as i32,
        y: j.num("y") as i32,
        scale: j.num("scale"),
        transform: j.num("transform") as i32,
        disabled: j.bool("disabled"),
        available_modes: j
            .arr("availableModes")
            .iter()
            .filter_map(|m| match m {
                Json::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
    }
}

pub fn load_monitors() -> Result<Vec<Monitor>, String> {
    let mut mons: Vec<Monitor> = match json::parse(&request("j/monitors all")?)? {
        Json::Arr(a) => a.iter().map(monitor).collect(),
        _ => return Err("unexpected reply from Hyprland".into()),
    };
    for m in &mut mons {
        if m.width == 0 || m.height == 0 {
            if let Some((w, h, hz)) = m.available_modes.first().and_then(|s| parse_mode(s)) {
                m.width = w;
                m.height = h;
                m.refresh_rate = hz;
            } else {
                m.width = 1920;
                m.height = 1080;
                m.refresh_rate = 60.0;
            }
        }
        if m.scale <= 0.0 {
            m.scale = 1.0;
        }
    }
    mons.sort_by_key(|m| m.id);
    Ok(mons)
}

pub fn normalize(mons: &mut [Monitor]) {
    let enabled: Vec<&Monitor> = mons.iter().filter(|m| !m.disabled).collect();
    if enabled.is_empty() {
        return;
    }
    let min_x = enabled.iter().map(|m| m.x).min().unwrap();
    let min_y = enabled.iter().map(|m| m.y).min().unwrap();
    for m in mons.iter_mut() {
        m.x -= min_x;
        m.y -= min_y;
    }
}

fn is_ok(reply: &str) -> bool {
    reply
        .lines()
        .all(|l| l.trim().is_empty() || l.trim() == "ok")
}

pub fn apply(mons: &[Monitor]) -> Result<(), String> {
    let lua: Vec<String> = mons.iter().map(Monitor::to_lua).collect();
    let mut reply = request(&format!("eval {}", lua.join(" ")))?;
    if !is_ok(&reply) && (reply.contains("unknown request") || reply.contains("invalid command")) {
        let cmds: Vec<String> = mons
            .iter()
            .map(|m| format!("keyword monitor {}", m.to_keyword()))
            .collect();
        reply = request(&format!("[[BATCH]]{}", cmds.join(";")))?;
    }
    if is_ok(&reply) {
        Ok(())
    } else {
        Err(reply.trim().to_string())
    }
}

fn hypr_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hypr")
}

pub fn uses_lua_config() -> bool {
    hypr_dir().join("hyprland.lua").exists()
}

pub fn profile_path() -> PathBuf {
    hypr_dir().join(if uses_lua_config() {
        "monitors.lua"
    } else {
        "monitors.conf"
    })
}

pub fn save_profile(mons: &[Monitor]) -> Result<PathBuf, String> {
    let path = profile_path();
    let mut body = String::new();
    if uses_lua_config() {
        body.push_str("-- Generated by fast-hyprmon. Load it from hyprland.lua with:\n");
        body.push_str("--   dofile(os.getenv(\"HOME\") .. \"/.config/hypr/monitors.lua\")\n");
        for m in mons {
            if !m.description.is_empty() {
                body.push_str(&format!("-- {}\n", m.description));
            }
            body.push_str(&m.to_lua());
            body.push('\n');
        }
        body.push_str(
            "hl.monitor({ output = \"\", mode = \"preferred\", position = \"auto\", scale = 1 })\n",
        );
    } else {
        body.push_str("# Generated by fast-hyprmon. Add `source = ~/.config/hypr/monitors.conf` to hyprland.conf.\n");
        for m in mons {
            if !m.description.is_empty() {
                body.push_str(&format!("# {}\n", m.description));
            }
            body.push_str(&format!("monitor = {}\n", m.to_keyword()));
        }
        body.push_str("monitor = ,preferred,auto,1\n");
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path)
}
