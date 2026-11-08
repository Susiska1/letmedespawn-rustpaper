// LetMeDespawn — RustPaper port of frikinjay's LetMeDespawn plugin.
// Prevents accidental persistent mobs: drops picked-up items on natural despawn,
// lets spawner mobs and block-carrying endermen despawn normally.
use horizon_bukkit::entity::Entity;
use horizon_bukkit::entities::Enderman;
use horizon_bukkit::fam_entity::MaterialData;
use horizon_bukkit::fam_ev_entity::Actor;
use horizon_bukkit::persistence::NamespacedKey;
use horizon_bukkit::prelude::*;
use std::collections::HashMap;
use std::sync::{Mutex, atomic::{AtomicU64, Ordering}};

// ── Constants ────────────────────────────────────────────────────────────────

const IGNORE_TAG: &str = "letmedespawn.ignore";
const VANILLA_IDLE_TICKS: i32 = 600;
const EQUIPMENT_SLOTS: &[&str] = &["HAND", "OFF_HAND", "HEAD", "CHEST", "LEGS", "FEET"];

// ── Settings ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct Settings {
    mob_names: Vec<String>,
    persistence_enablers: Vec<String>,
    standard_spawner_despawn: bool,
    trial_spawner_despawn: bool,
    check_interval_ticks: u64,
    enderman_soft_range_sq: f64,
    enderman_hard_range_sq: f64,
    enderman_soft_despawn_chance: f64,
}

// ── Global state ─────────────────────────────────────────────────────────────

static SETTINGS: Mutex<Option<Settings>> = Mutex::new(None);
/// uuid → (world_name, x, y, z)
static PLAYER_POSITIONS: Mutex<Option<HashMap<String, (String, f64, f64, f64)>>> = Mutex::new(None);

#[derive(Clone, Debug)]
struct Watch {
    entity: Entity,
    spawner: bool,
    trial: bool,
    locked: bool,
    seed: u32,
}
/// entity_id → Watch
static WATCHES: Mutex<Option<HashMap<u64, Watch>>> = Mutex::new(None);
static DESPAWN_COUNT: AtomicU64 = AtomicU64::new(0);
// ── Settings helpers ─────────────────────────────────────────────────────────

fn get_settings() -> Settings {
    SETTINGS.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref().cloned().unwrap_or_default()
}

fn load_settings(plugin: Plugin) {
    plugin.save_default_config(include_str!("../config.yml"));
    let config = plugin.get_config();
    let mob_names: Vec<String> = config.get_string_list("mobNames")
        .into_iter().filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_lowercase()).collect();
    let persistence_enablers: Vec<String> = config.get_string_list("persistenceEnablers")
        .into_iter().filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_lowercase()).collect();
    let standard = config.get_boolean_or("enableStandardSpawnerDespawn", true);
    let trial    = config.get_boolean_or("enableTrialSpawnerDespawn", true);
    let interval = config.get_int_or("advanced.check-interval-ticks", 20).clamp(1, 1200) as u64;
    let soft = config.get_int_or("advanced.enderman-despawn-range.soft", 32).clamp(0, 1024) as f64;
    let hard = config.get_int_or("advanced.enderman-despawn-range.hard", 128)
        .clamp(soft as i32, 1024) as f64;
    let soft_chance = 1.0 - 0.99875_f64.powi(interval as i32);
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Settings {
        mob_names, persistence_enablers,
        standard_spawner_despawn: standard, trial_spawner_despawn: trial,
        check_interval_ticks: interval,
        enderman_soft_range_sq:      soft * soft,
        enderman_hard_range_sq:      hard * hard,
        enderman_soft_despawn_chance: soft_chance,
    });
}

fn mutate_list(plugin: Plugin, path: &str, f: impl FnOnce(&mut Vec<String>) -> bool) -> bool {
    let mut config = plugin.get_config();
    let mut list = config.get_string_list(path);
    if !f(&mut list) { return false; }
    config.set(path, Value::Array(list.into_iter().map(Value::String).collect()));
    plugin.save_config(&config);
    load_settings(plugin);
    true
}

fn add_to_list(plugin: Plugin, path: &str, value: &str) -> bool {
    let v = value.to_owned();
    mutate_list(plugin, path, |list| {
        if list.contains(&v) { return false; }
        list.push(v); true
    })
}

fn remove_from_list(plugin: Plugin, path: &str, value: &str) -> bool {
    let v = value.to_owned();
    mutate_list(plugin, path, |list| {
        let before = list.len();
        list.retain(|x| x != &v);
        list.len() != before
    })
}

fn set_boolean_setting(plugin: Plugin, path: &str, value: bool) {
    let mut config = plugin.get_config();
    config.set(path, Value::Bool(value));
    plugin.save_config(&config);
    load_settings(plugin);
}
// ── Player positions ─────────────────────────────────────────────────────────

fn update_player_pos(player: &Player) {
    let uuid = player.get_unique_id();
    if player.get_game_mode() == "SPECTATOR" {
        if let Some(m) = PLAYER_POSITIONS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            m.remove(&uuid);
        }
        return;
    }
    if let loc = player.get_location() {
        let world_name = player.get_world().get_name().to_owned();
        PLAYER_POSITIONS.lock().unwrap_or_else(|e| e.into_inner())
            .get_or_insert_default()
            .insert(uuid, (world_name, loc.get_x(), loc.get_y(), loc.get_z()));
    }
}

fn forget_player(player: &Player) {
    let uuid = player.get_unique_id();
    if let Some(m) = PLAYER_POSITIONS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&uuid);
    }
}

/// Squared distance to the nearest tracked player in `world_name`, or -1.0 if none.
fn nearest_dist_sq(world_name: &str, x: f64, y: f64, z: f64) -> f64 {
    let guard = PLAYER_POSITIONS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = guard.as_ref() else { return -1.0; };
    let mut best: f64 = -1.0;
    for (wn, px, py, pz) in map.values() {
        if wn != world_name { continue; }
        let d = (px - x).powi(2) + (py - y).powi(2) + (pz - z).powi(2);
        if best < 0.0 || d < best { best = d; }
    }
    best
}

// ── PDC helpers ──────────────────────────────────────────────────────────────

fn pdc_key(suffix: &str) -> NamespacedKey {
    NamespacedKey::from_string(&format!("letmedespawn:{suffix}")).expect("valid key")
}
fn pdc_has(e: &Entity, k: &NamespacedKey) -> bool { e.pdc_get(k).is_some() }
fn pdc_flag(e: &Entity, k: &NamespacedKey)         { e.pdc_set(k, &json!(1)); }
fn pdc_bool_get(e: &Entity, k: &NamespacedKey) -> bool {
    e.pdc_get(k).and_then(|v| v.as_i64()).map_or(false, |n| n != 0)
}
fn pdc_bool_set(e: &Entity, k: &NamespacedKey, v: bool) {
    let n: i64 = if v { 1 } else { 0 };
    e.pdc_set(k, &json!(n));
}
// ── Core logic ───────────────────────────────────────────────────────────────

fn is_excluded(e: &Entity) -> bool {
    e.get_scoreboard_tags().iter().any(|t| t == IGNORE_TAG)
}
fn keeps_pickup_persistence(e: &Entity, s: &Settings) -> bool {
    e.get_custom_name().is_some() || s.mob_names.contains(&e.get_type())
}
fn mark_spawn_source(e: &Entity, trial: bool) {
    if trial { pdc_flag(e, &pdc_key("trial_spawner")); }
    else     { pdc_flag(e, &pdc_key("spawner")); }
}
fn record_trial_pos(e: &Entity, x: i32, y: i32, z: i32) {
    e.pdc_set(&pdc_key("trial_spawner_pos"), &json!([x, y, z]));
}
fn has_picked_items(e: &Entity) -> bool {
    pdc_has(e, &pdc_key("picked_slots")) || pdc_bool_get(e, &pdc_key("enabler_lock"))
}
fn release(e: &Entity) {
    e.set_remove_when_far_away(true);
    pdc_flag(e, &pdc_key("released"));
}
fn release_spawner_persistence(e: &Entity, s: &Settings) {
    // Only release if mob is currently persistent (not already remove_when_far_away)
    // and isn't named or in the kept-persistent list.
    if !e.get_remove_when_far_away()
        && e.get_custom_name().is_none()
        && !s.mob_names.contains(&e.get_type())
    { release(e); }
}

/// Snapshot current equipment: slot → Option<item type key>.
fn snapshot_equip(e: &Entity) -> HashMap<&'static str, Option<String>> {
    let eq = e.get_equipment();
    EQUIPMENT_SLOTS.iter().map(|&slot| {
        (slot, eq.get_item(slot).map(|s| s.get_type().key().to_owned()))
    }).collect()
}

/// Called 1 tick after a pickup/dispense. Returns true if mob should be watched.
fn after_equip(
    e: &Entity,
    item_type: &str,
    before: HashMap<&'static str, Option<String>>,
    was_persistent: bool,
    s: &Settings,
) -> bool {
    if is_excluded(e) { return false; }
    let eq = e.get_equipment();
    let mut equipped = false;
    for &slot in EQUIPMENT_SLOTS {
        let now_key = eq.get_item(slot).map(|st| st.get_type().key().to_owned());
        if now_key.is_none() { continue; }
        if before.get(slot).and_then(|v| v.as_deref()) == now_key.as_deref() { continue; }
        if now_key.as_deref() == Some(item_type) {
            mark_picked(e, slot, item_type);
            equipped = true;
        }
    }
    if !equipped { return false; }
    if !was_persistent && e.get_remove_when_far_away() && !keeps_pickup_persistence(e, s) {
        release(e);
    }
    update_enabler_lock(e, s)
}
fn mark_picked(e: &Entity, slot: &str, item_type: &str) {
    let key = pdc_key("picked_slots");
    let existing = e.pdc_get(&key).and_then(|v| v.as_str().map(str::to_owned));
    let mut out = format!("{slot}={item_type}");
    if let Some(ref old) = existing {
        for entry in old.split(',').filter(|s| !s.is_empty()) {
            let es = entry.split_once('=').map_or(entry, |(s, _)| s);
            if es != slot { out.push(','); out.push_str(entry); }
        }
    }
    e.pdc_set(&key, &json!(out));
}

fn holds_persistence_enabler(e: &Entity, s: &Settings) -> bool {
    if s.persistence_enablers.is_empty() { return false; }
    let Some(picked) = e.pdc_get(&pdc_key("picked_slots"))
        .and_then(|v| v.as_str().map(str::to_owned)) else { return false; };
    let eq = e.get_equipment();
    for entry in picked.split(',').filter(|s| !s.is_empty()) {
        let Some((slot, rec)) = entry.split_once('=') else { continue; };
        let Some(stack) = eq.get_item(slot) else { continue; };
        let ak = stack.get_type().key();
        if ak == rec && s.persistence_enablers.contains(&ak.to_owned()) { return true; }
    }
    false
}

/// Returns true if the mob is currently kept persistent by a persistence-enabler item.
fn update_enabler_lock(e: &Entity, s: &Settings) -> bool {
    let lock = pdc_key("enabler_lock");
    let was_locked = pdc_bool_get(e, &lock);
    if holds_persistence_enabler(e, s) {
        if !was_locked && e.get_remove_when_far_away() {
            e.set_remove_when_far_away(false);
            pdc_bool_set(e, &lock, true);
            return true;
        }
        return was_locked;
    }
    if was_locked {
        pdc_bool_set(e, &lock, false);
        if !keeps_pickup_persistence(e, s) { release(e); }
    }
    false
}

// ── Despawn handling ─────────────────────────────────────────────────────────

fn on_despawn(e: &Entity) {
    if pdc_has(e, &pdc_key("released")) {
        DESPAWN_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    drop_picked_items(e);
}

fn drop_picked_items(e: &Entity) {
    let Some(loc) = e.get_location() else { return; };
    let world = e.get_world();
    // Drop any items the mob picked up
    if let Some(picked) = e.pdc_get(&pdc_key("picked_slots"))
        .and_then(|v| v.as_str().map(str::to_owned))
    {
        let eq = e.get_equipment();
        for entry in picked.split(',').filter(|s| !s.is_empty()) {
            let Some((slot, rec)) = entry.split_once('=') else { continue; };
            let Some(stack) = eq.get_item(slot) else { continue; };
            if stack.get_type().key() != rec { continue; }
            if stack.has_enchant("minecraft:vanishing_curse") { continue; }
            world.drop_item(&loc, &stack);
            let air = ItemStack::new(Material::of("minecraft:air"), 0);
            eq.set_item(slot, &air);
        }
    }
    // Enderman: drop carried block as an item
    if e.get_type() == "minecraft:enderman" {
        let enderman = e.cast::<Enderman>();
        let carried = enderman.get_carried_material();
        if !carried.get_item_type().is_air() {
            world.drop_item(&loc, &ItemStack::new(carried.get_item_type().clone(), 1));
            enderman.set_carried_material(&MaterialData::new(Material::of("minecraft:air")));
        }
    }
}
// ── Enderman ticking ─────────────────────────────────────────────────────────

/// Simple xorshift64 PRNG.
fn rand_f64() -> f64 {
    static SEED: AtomicU64 = AtomicU64::new(2463534242);
    let mut x = SEED.load(Ordering::Relaxed);
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    SEED.store(x, Ordering::Relaxed);
    (x >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Tick an enderman that may be carrying a block. Returns true if still needs watching.
fn tick_enderman(e: &Entity, s: &Settings) -> bool {
    if e.get_type() != "minecraft:enderman" { return false; }
    let end = e.cast::<Enderman>();
    if end.get_carried_material().get_item_type().is_air() { return false; }
    if !e.get_remove_when_far_away() || e.get_custom_name().is_some() || e.get_vehicle().is_some() {
        return true;
    }
    let Some(loc) = e.get_location() else { return true; };
    let wname = e.get_world().get_name().to_owned();
    let dist = nearest_dist_sq(&wname, loc.get_x(), loc.get_y(), loc.get_z());
    if dist < 0.0 { return true; }
    let despawn = dist > s.enderman_hard_range_sq
        || (dist > s.enderman_soft_range_sq
            && e.get_no_action_ticks() > VANILLA_IDLE_TICKS
            && rand_f64() < s.enderman_soft_despawn_chance);
    if despawn {
        drop_picked_items(e);
        e.remove();
        DESPAWN_COUNT.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    true
}

// ── Trial spawner tracking ────────────────────────────────────────────────────

/// Returns true if the mob is still being tracked by its trial spawner.
fn is_tracked_by_trial_spawner(e: &Entity) -> bool {
    let Some(val) = e.pdc_get(&pdc_key("trial_spawner_pos")) else { return true; };
    let Some(arr) = val.as_array() else { return true; };
    if arr.len() < 3 { return true; }
    let (sx, sy, sz) = (
        arr[0].as_i64().unwrap_or(0),
        arr[1].as_i64().unwrap_or(0),
        arr[2].as_i64().unwrap_or(0),
    );
    let Some(loc) = e.get_location() else { return true; };
    let dx = loc.get_x() as i64 - sx;
    let dy = loc.get_y() as i64 - sy;
    let dz = loc.get_z() as i64 - sz;
    if dx * dx + dy * dy + dz * dz > 2209 { return false; }  // > 47 blocks
    // Check if the spawner block still exists
    let block = e.get_world().get_block_at(sx as i32, sy as i32, sz as i32);
    block.get_state().get_type().key() == "minecraft:trial_spawner"
}
// ── Mob watcher ──────────────────────────────────────────────────────────────

fn consider_mob(e: &Entity, s: &Settings) {
    if is_excluded(e) { return; }
    let spawner  = pdc_has(e, &pdc_key("spawner"));
    let trial    = pdc_has(e, &pdc_key("trial_spawner"));
    let locked   = has_picked_items(e) && update_enabler_lock(e, s);
    let carrying = e.get_type() == "minecraft:enderman"
        && !e.cast::<Enderman>().get_carried_material().get_item_type().is_air();
    if spawner || trial || locked || carrying {
        watch_mob(e, spawner, trial, locked);
    }
}

fn watch_mob(e: &Entity, spawner: bool, trial: bool, locked: bool) {
    let id = e.get_entity_id();
    let mut g = WATCHES.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_default();
    if let Some(w) = map.get_mut(&id) {
        w.spawner |= spawner; w.trial |= trial; w.locked |= locked;
    } else {
        let seed = ((id ^ (id >> 16)) & 0xFFFF_FFFF) as u32;
        map.insert(id, Watch { entity: e.clone(), spawner, trial, locked, seed });
    }
}

/// Tick one watch entry. Returns true if it should remain in the map.
fn tick_watch(w: &mut Watch, s: &Settings) -> bool {
    if !w.entity.is_valid()
        || w.entity.get_scoreboard_tags().iter().any(|t| t == IGNORE_TAG)
    { return false; }
    if w.locked { w.locked = update_enabler_lock(&w.entity, s); }
    if !w.locked {
        if w.spawner && s.standard_spawner_despawn {
            release_spawner_persistence(&w.entity, s);
        }
        if w.trial && s.trial_spawner_despawn
            && !is_tracked_by_trial_spawner(&w.entity)
        {
            release_spawner_persistence(&w.entity, s);
        }
    }
    let carrying = tick_enderman(&w.entity, s);
    w.locked || w.spawner || w.trial || carrying
}

/// Called every server tick; distributes work across `check_interval_ticks` ticks.
fn tick_all_watches() {
    static TICK: AtomicU64 = AtomicU64::new(0);
    let tick = TICK.fetch_add(1, Ordering::Relaxed);
    let s = get_settings();
    let interval = s.check_interval_ticks.max(1);
    let slice = tick % interval;
    let mut g = WATCHES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = g.as_mut() {
        map.retain(|_, w| w.seed as u64 % interval != slice || tick_watch(w, &s));
    }
}
// ── Command handler ───────────────────────────────────────────────────────────

const SUBCOMMANDS: &[&str] = &[
    "add", "remove", "addPersistenceEnabler", "removePersistenceEnabler",
    "standardSpawnerDespawn", "trialSpawnerDespawn", "reload", "info",
];

fn cmd_usage(sender: &CommandSender, label: &str) {
    sender.send_message("§bLetMeDespawn commands:");
    sender.send_message(&format!("§7/{label} add|remove <mob>"));
    sender.send_message(&format!("§7/{label} addPersistenceEnabler|removePersistenceEnabler <item>"));
    sender.send_message(&format!("§7/{label} standardSpawnerDespawn|trialSpawnerDespawn <true|false>"));
    sender.send_message(&format!("§7/{label} reload|info"));
}

fn cmd_reply(sender: &CommandSender, changed: bool, done: &str, unchanged: &str) {
    if changed { broadcast_message(done); }
    else { sender.send_message(&format!("§c{unchanged}")); }
}

fn cmd_set_bool(plugin: Plugin, sender: &CommandSender, label: &str,
    name: &str, path: &str, display: &str, value: &str)
{
    let parsed = match value.to_lowercase().as_str() {
        "true"|"on"|"yes" => Some(true),
        "false"|"off"|"no" => Some(false),
        _ => None,
    };
    let Some(b) = parsed else {
        sender.send_message(&format!("§cUsage: /{label} {name} <true|false>")); return;
    };
    set_boolean_setting(plugin, path, b);
    broadcast_message(&format!("§b{display} set to: {b}"));
}

fn cmd_info(plugin: Plugin, sender: &CommandSender) {
    let s = get_settings();
    sender.send_message(&format!("§bLetMeDespawn §f{}", plugin.get_version()));
    sender.send_message(&format!("§7Mobs kept persistent on pickup: §f{:?}", s.mob_names));
    sender.send_message(&format!("§7Persistence enablers: §f{:?}", s.persistence_enablers));
    sender.send_message(&format!("§7Standard spawner despawn: §f{}", s.standard_spawner_despawn));
    sender.send_message(&format!("§7Trial spawner despawn: §f{}", s.trial_spawner_despawn));
    let count = WATCHES.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref().map_or(0, HashMap::len);
    sender.send_message(&format!("§7Mobs being watched: §f{count}"));
    sender.send_message(&format!("§7Despawned since startup: §f{}",
        DESPAWN_COUNT.load(Ordering::Relaxed)));
}

fn handle_command(plugin: Plugin, sender: &CommandSender, label: &str, args: &[&str]) -> bool {
    if args.is_empty() { cmd_usage(sender, label); return true; }
    let sub = args[0].to_lowercase();
    let val = if args.len() > 1 { args[1..].join(" ").trim().to_lowercase() } else { String::new() };
    match sub.as_str() {
        "add" => {
            if val.is_empty() { sender.send_message(&format!("§cUsage: /{label} add <mob>")); return true; }
            cmd_reply(sender, add_to_list(plugin, "mobNames", &val),
                &format!("§bAdded '{val}' to LetMeDespawn configuration."),
                &format!("Mob '{val}' is already in the configuration."));
        }
        "remove" => {
            if val.is_empty() { sender.send_message(&format!("§cUsage: /{label} remove <mob>")); return true; }
            cmd_reply(sender, remove_from_list(plugin, "mobNames", &val),
                &format!("§6Removed '{val}' from LetMeDespawn configuration."),
                &format!("Mob '{val}' is not in the configuration."));
        }
        "addpersistenceenabler" => {
            if val.is_empty() { sender.send_message(&format!("§cUsage: /{label} addPersistenceEnabler <item>")); return true; }
            cmd_reply(sender, add_to_list(plugin, "persistenceEnablers", &val),
                &format!("§bAdded '{val}' to persistence enablers."),
                &format!("Item '{val}' is already a persistence enabler."));
        }
        "removepersistenceenabler" => {
            if val.is_empty() { sender.send_message(&format!("§cUsage: /{label} removePersistenceEnabler <item>")); return true; }
            cmd_reply(sender, remove_from_list(plugin, "persistenceEnablers", &val),
                &format!("§6Removed '{val}' from persistence enablers."),
                &format!("Item '{val}' is not a persistence enabler."));
        }
        "standardspawnerdespawn" => cmd_set_bool(plugin, sender, label,
            "standardSpawnerDespawn", "enableStandardSpawnerDespawn", "Standard spawner despawn", &val),
        "trialspawnerdespawn" => cmd_set_bool(plugin, sender, label,
            "trialSpawnerDespawn", "enableTrialSpawnerDespawn", "Trial spawner despawn", &val),
        "reload" => { load_settings(plugin); broadcast_message("§bReloaded the LetMeDespawn configuration."); }
        "info" => cmd_info(plugin, sender),
        _ => cmd_usage(sender, label),
    }
    true
}

fn tab_complete(args: &[&str]) -> Vec<String> {
    if args.len() == 1 {
        let p = args[0].to_lowercase();
        return SUBCOMMANDS.iter().filter(|s| s.to_lowercase().starts_with(&p))
            .map(|s| s.to_string()).collect();
    }
    if args.len() == 2 {
        let s = get_settings(); let p = args[1].to_lowercase();
        return match args[0].to_lowercase().as_str() {
            "remove" => s.mob_names.iter().filter(|v| v.starts_with(&p)).cloned().collect(),
            "removepersistenceenabler" => s.persistence_enablers.iter()
                .filter(|v| v.starts_with(&p)).cloned().collect(),
            "standardspawnerdespawn"|"trialspawnerdespawn" =>
                ["true","false"].iter().filter(|v| v.starts_with(p.as_str())).map(|s| s.to_string()).collect(),
            _ => vec![],
        };
    }
    vec![]
}
// ── Plugin lifecycle ──────────────────────────────────────────────────────────

fn on_enable(plugin: Plugin) {
    load_settings(plugin);

    // Scan entities already loaded at startup
    let s = get_settings();
    for world in get_worlds() {
        for entity in world.get_entities() {
            if entity.is_valid() { consider_mob(&entity, &s); }
        }
    }

    // Update player positions every 10 ticks
    plugin.run_task_timer(1, 10, move || {
        for p in get_online_players() { update_player_pos(&p); }
    });

    // Watcher tick every tick
    plugin.run_task_timer(1, 1, || tick_all_watches());

    // Spawner/trial-spawner spawn: mark mob and consider it
    plugin.register_event(move |e: &mut CreatureSpawnEvent| {
        let reason = e.get_spawn_reason();
        let trial = reason == "TRIAL_SPAWNER";
        if !trial && reason != "SPAWNER" { return; }
        let Some(entity) = e.get_entity() else { return; };
        if is_excluded(&entity) { return; }
        mark_spawn_source(&entity, trial);
        consider_mob(&entity, &get_settings());
    });

    // Trial spawner spawn: record spawner block position for proximity check
    plugin.register_event(move |e: &mut TrialSpawnerSpawnEvent| {
        let Some(Actor::Entity(entity)) = e.get_entity() else { return; };
        let Some(state) = e.get_trial_spawner() else { return; };
        let block = state.get_block();
        record_trial_pos(&entity, block.get_x(), block.get_y(), block.get_z());
    });

    // Mob picks up a dropped item: deferred equip-slot diff
    plugin.register_event(move |e: &mut EntityPickupItemEvent| {
        let Some(Actor::Entity(entity)) = e.get_entity() else { return; };
        if is_excluded(&entity) { return; }
        let Some(item) = e.get_item() else { return; };
        let itype = item.get_type().key().to_owned();
        let before = snapshot_equip(&entity);
        let was_persistent = !entity.get_remove_when_far_away();
        plugin.run_task_later(1, move || {
            let s = get_settings();
            if after_equip(&entity, &itype, before.clone(), was_persistent, &s) {
                consider_mob(&entity, &s);
            }
        });
    });

    // Dispenser equips armor to a mob: same logic
    plugin.register_event(move |e: &mut BlockDispenseArmorEvent| {
        let Some(entity) = e.get_target_entity() else { return; };
        if is_excluded(&entity) { return; }
        let Some(item) = e.get_item() else { return; };
        let itype = item.get_type().key().to_owned();
        let before = snapshot_equip(&entity);
        let was_persistent = !entity.get_remove_when_far_away();
        plugin.run_task_later(1, move || {
            let s = get_settings();
            if after_equip(&entity, &itype, before.clone(), was_persistent, &s) {
                consider_mob(&entity, &s);
            }
        });
    });

    // Enderman picks up / places a block
    plugin.register_event(move |e: &mut EntityChangeBlockEvent| {
        let Some(entity) = e.get_entity() else { return; };
        if entity.get_type() != "minecraft:enderman" { return; }
        plugin.run_task_later(1, move || consider_mob(&entity, &get_settings()));
    });

    // Entities loaded from disk (chunk load): consider each one
    plugin.register_event(move |e: &mut EntitiesLoadEvent| {
        let s = get_settings();
        for entity in e.get_entities() {
            if entity.is_valid() { consider_mob(entity, &s); }
        }
    });

    // Natural despawn: drop picked items, update stats
    plugin.register_event(move |e: &mut EntityRemoveEvent| {
        let Some(entity) = e.get_entity() else { return; };
        // "DISCARD" = natural game despawn (not death, not chunk unload, not portal)
        if entity.get_remove_event_cause() != Some("DISCARD") { return; }
        if is_excluded(&entity) { return; }
        on_despawn(&entity);
    });

    // Player join/quit: maintain position cache for enderman distance checks
    plugin.register_event(move |e: &mut PlayerJoinEvent| update_player_pos(&e.get_player()));
    plugin.register_event(move |e: &mut PlayerQuitEvent| forget_player(&e.get_player()));

    // /letmedespawn (alias /lmd)
    plugin.set_executor("letmedespawn",
        move |sender, label, args| handle_command(plugin, sender, label, args));
    plugin.set_tab_completer("letmedespawn",
        |_, _, args| tab_complete(args));

    plugin.info("LetMeDespawn enabled");
}

fn on_disable(plugin: Plugin) {
    *WATCHES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *PLAYER_POSITIONS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    plugin.info("LetMeDespawn disabled");
}

bukkit_plugin!(on_enable, on_disable);

