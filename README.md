\# LetMeDespawn (RustPaper Port)



A high-performance Rust port of the popular Minecraft optimization plugin \[Let Me Despawn](https://modrinth.com/plugin/lmd/versions) (originally by frikinjay), rewritten specifically for the \*\*RustPaper\*\* server platform using the RustPaper SDK.
[![Download RustPaper](https://img.shields.io/badge/Download%20rustpaper-8A2BE2)](https://github.com/SaphirePay/rustpaper)




Like the Java original, this plugin drastically reduces entity lag by making sure mobs that pick up items can still naturally despawn without losing the items they held.



\---



\## 🌟 Features \& Command Support



\* \*\*Smart Despawning:\*\* Mobs that pick up items remain despawnable.

\* \*\*Item Preservation:\*\* Items held by naturally despawned mobs are dropped safely onto the ground.

\* \*\*Spawner Controls:\*\* Allows mobs from standard spawners and trial spawners to safely despawn.

\* \*\*Endermen Logic:\*\* Allows block-carrying Endermen to despawn based on distance.

\* \*\*Exclusions:\*\* Named mobs and mobs with the `letmedespawn.ignore` tag are strictly ignored.

\* \*\*Full Command Suite:\*\* All 8 subcommands (`/letmedespawn` or `/lmd`) are fully implemented:

&#x20; \* `add`, `remove`, `addPersistenceEnabler`, `removePersistenceEnabler`, `standardSpawnerDespawn`, `trialSpawnerDespawn`, `reload`, `info`.



\---



\## 🛠️ Porting Decisions \& Technical Nuances (Java vs. Rust)



Due to structural differences between the Bukkit Java API and the RustPaper SDK, several architectural adaptations were made during development:



\### 1. Persistent Data Container (PDC) Toggling

\* \*\*Java:\*\* Toggles the `enabler\_lock` flag by physically adding or removing the key via `pdc.remove()`.

\* \*\*Rust:\*\* The RustPaper SDK currently lacks an explicit `pdc\_remove` method for entities. Instead, this port stores a boolean integer (`1` for true, `0` for false) and reads it back to toggle the flag state. Presence-only flags (`spawner`, `trial\_spawner`, `released`) are verified using `pdc\_get().is\_some()`.



\### 2. Despawn Detection Hook

\* \*\*Java:\*\* Listens directly to `EntityRemoveEvent.Cause.DESPAWN`.

\* \*\*Rust:\*\* The SDK event currently only explicitly differentiates between `DEATH` and `UNLOAD`. To capture natural despawns, this port checks `Entity::get\_remove\_event\_cause()`, filtering for the `"DISCARD"` string value (which maps to despawns, merges, and pickups in the SDK framework).



\### 3. Tab Completion Limitations

\* \*\*Java:\*\* Scans the entire internal Minecraft registry (`Registry.ENTITY\_TYPE`) to autocomplete any valid mob type.

\* \*\*Rust:\*\* Because registry iteration isn't exposed in the same way by the SDK, tab completion is localized to subcommand names and the entity types explicitly listed in your `config.yml`.



\### 4. Folia, bStats, and Peaceful-Mode Cleanup

\* \*\*Folia \& bStats:\*\* Completely dropped. Thread scheduling is handled natively by the RustPaper SDK, and there is no Rust equivalent for bStats metrics yet.

\* \*\*Enemy-on-Peaceful Skip:\*\* Removed. The SDK cannot reliably isolate hostile mobs via interfaces at runtime, which previously only affected despawn counts on Peaceful difficulty.



\---



\## 🚀 Installation \& Usage



\### Method 1: Using Precompiled Releases (Quickest)

1\. Go to the \*\*Releases\*\* tab on the right side of this GitHub repository.

2\. Download the precompiled binary for your operating system (e.g., `letmedespawn.dll` for Windows or `.so` for Linux).

3\. Drop the file into your server's `plugins/` directory.

4\. Restart or start your RustPaper server.



\### Method 2: Compiling from Source (Using Cargo)

If you prefer to build the binary yourself or are modifying the source code, you can easily compile it using the Rust toolchain.



\#### 📦 Project Structure \& SDK Paths

By default, this plugin is configured to be compiled inside the \*\*rustpaper\*\* workspace environment. 

\* \*\*Standard Setup:\*\* Clone or move this `letmedespawn` folder directly into the `rustpaper/plugins/` directory of your RustPaper server files. The `Cargo.toml` will automatically find the SDK using the relative path `../../sdk/horizon-bukkit`.

\* \*\*The System32 Alternative:\*\* If you set up your `rustpaper` environment using an Administrator command prompt, your SDK might live in `C:\\Windows\\System32\\rustpaper\\sdk\\horizon-bukkit`. If you wish to build standalone from that directory, open `Cargo.toml`, comment out the relative path, and uncomment the absolute `System32` path line.



\#### Build Steps

1\. Navigate into your local plugin folder:

&#x20;  ```bash

&#x20;  cd rustpaper/plugins/letmedespawn

&#x20;  ```

2\. Compile the production release:

&#x20;  ```bash

&#x20;  cargo build --release

&#x20;  ```

3\. Find your compiled library file inside the `target/release/` directory (e.g., `letmedespawn.dll` on Windows or `libletmedespawn.so` on Linux).

4\. Copy the file into your active server's `plugins/` folder.



\---



\## 📜 License \& Credits



\* Original Java plugin by \*\*frikinjay\*\* (\[Let Me Despawn on Modrinth](https://modrinth.com)).

\* This port is distributed under the \*\*MIT License\*\*, remaining completely free and open-source in compliance with the original project's terms.



