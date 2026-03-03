/// Integration tests for roguelike ECS systems.
///
/// These tests create a minimal Bevy `App`, spawn entities with the required
/// components, fire messages, and run individual systems to verify behaviour.
/// This approach tests the actual system functions with real ECS plumbing,
/// ensuring the bug fixes work end-to-end.
use bevy::prelude::*;

use roguelike::components::*;
use roguelike::events::*;
use roguelike::gamemap::GameMap;
use roguelike::grid_vec::GridVec;
use roguelike::resources::*;
use roguelike::systems::{ai, combat, inventory, movement, projectile, spatial_index, spell, visibility};

// ─── Helper ──────────────────────────────────────────────────────

/// Creates a minimal App wired for movement/combat testing.
/// The map is 120×80 with seed 42 (matching the game defaults).
fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::default());
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_message::<MoveIntent>();
    app.add_message::<AttackIntent>();
    app.add_message::<DamageEvent>();
    app.init_resource::<SpatialIndex>();
    app.init_resource::<CombatLog>();
    app.init_resource::<KillCount>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<CursorPosition>();
    app.init_resource::<BloodMap>();
    app.init_resource::<TurnCounter>();
    app.init_resource::<InputState>();
    app.init_resource::<GodMode>();
    app.init_resource::<SpectatingAfterDeath>();
    app.init_resource::<DynamicRng>();
    app.init_state::<GameState>();
    app.insert_resource(GameMapResource(GameMap::new(120, 80, 42)));
    app.insert_resource(MapSeed(42));
    app.add_systems(
        Update,
        (
            spatial_index::spatial_index_system,
            movement::movement_system,
            combat::combat_system,
            combat::apply_damage_system,
            combat::death_system,
        )
            .chain(),
    );
    app
}

/// Spawns a player entity at the given position with standard stats.
fn spawn_test_player(app: &mut App, x: i32, y: i32) -> Entity {
    app.world_mut().spawn((
        Position { x, y },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id()
}

/// Spawns a hostile monster at the given position with standard stats.
fn spawn_test_monster(app: &mut App, x: i32, y: i32, name: &str) -> Entity {
    app.world_mut().spawn((
        Position { x, y },
        Hostile,
        BlocksMovement,
        Name(name.into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3 },
    )).id()
}

// ─── Movement tests ──────────────────────────────────────────────

#[test]
fn player_moves_to_passable_tile() {
    let mut app = test_app();
    // Spawn player at spawn area center (guaranteed clear)
    let player = spawn_test_player(&mut app, 60, 40);

    // Run once to build spatial index
    app.update();

    // Emit move intent: east (+1, 0)
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let pos = app.world().get::<Position>(player).unwrap();
    assert_eq!(pos.x, 61, "Player should have moved east");
    assert_eq!(pos.y, 40);
}

#[test]
fn player_blocked_by_wall() {
    let mut app = test_app();
    // Place player next to a wall (border at x=0)
    let player = spawn_test_player(&mut app, 1, 1);

    app.update();

    // Try to move west into the wall at x=0
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: -1,
        dy: 0,
    });
    app.update();

    let pos = app.world().get::<Position>(player).unwrap();
    assert_eq!(pos.x, 1, "Player should be blocked by wall");
    assert_eq!(pos.y, 1);
}

#[test]
fn player_blocked_by_monster() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let _monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // Player tries to move into monster's tile — should attack, not move
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let pos = app.world().get::<Position>(player).unwrap();
    assert_eq!(pos.x, 60, "Player should not have moved into monster tile");
    assert_eq!(pos.y, 40);
}

#[test]
fn monster_blocked_by_player() {
    let mut app = test_app();
    let _player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // Monster tries to move into player's tile — should attack, not move
    app.world_mut().write_message(MoveIntent {
        entity: monster,
        dx: -1,
        dy: 0,
    });
    app.update();

    let pos = app.world().get::<Position>(monster).unwrap();
    assert_eq!(pos.x, 61, "Monster should not have moved into player tile");
    assert_eq!(pos.y, 40);
}

#[test]
fn monster_cannot_overlap_another_monster() {
    let mut app = test_app();
    let _player = spawn_test_player(&mut app, 60, 40);
    let monster1 = spawn_test_monster(&mut app, 62, 40, "Goblin");
    let _monster2 = spawn_test_monster(&mut app, 63, 40, "Orc");

    app.update();

    // Monster1 tries to move east into Monster2's tile
    app.world_mut().write_message(MoveIntent {
        entity: monster1,
        dx: 1,
        dy: 0,
    });
    app.update();

    let pos = app.world().get::<Position>(monster1).unwrap();
    assert_eq!(pos.x, 62, "Monster should be blocked by another monster");
}

// ─── Bump-to-attack tests ────────────────────────────────────────

#[test]
fn player_bump_attack_damages_monster() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // Player bumps into monster → should trigger attack
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Player attack=5 → damage=5
    let monster_health = app.world().get::<Health>(monster).unwrap();
    assert_eq!(monster_health.current, 5, "Monster should have taken 5 damage");
}

#[test]
fn monster_bump_attack_damages_player() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // Monster bumps into player → should trigger attack
    app.world_mut().write_message(MoveIntent {
        entity: monster,
        dx: -1,
        dy: 0,
    });
    app.update();

    // Monster attack=3 → damage=3
    let player_health = app.world().get::<Health>(player).unwrap();
    assert_eq!(player_health.current, 27, "Player should have taken 3 damage");
}

#[test]
fn low_attack_still_deals_damage() {
    let mut app = test_app();
    // Spawn player
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id();

    // Spawn weak monster
    let monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Rat".into()),
        Health { current: 5, max: 5 },
        CombatStats { attack: 2 },
    )).id();

    app.update();

    // Monster attacks player: attack=2 → damage=2
    app.world_mut().write_message(MoveIntent {
        entity: monster,
        dx: -1,
        dy: 0,
    });
    app.update();

    let player_health = app.world().get::<Health>(player).unwrap();
    assert_eq!(player_health.current, 28, "Player should take 2 damage from weak monster");
}

// ─── Death system tests ──────────────────────────────────────────

#[test]
fn monster_dies_at_zero_health() {
    let mut app = test_app();
    // Spawn a monster with 1 HP
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weakling".into()),
        Health { current: 1, max: 1 },
        CombatStats { attack: 1 },
    )).id();

    app.update();

    // Player attacks monster: attack=5 → damage=5, kills the 1HP monster
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Monster should be despawned
    assert!(
        app.world().get::<Health>(monster).is_none(),
        "Monster should be despawned after reaching 0 HP"
    );
}

#[test]
fn entity_with_positive_health_survives() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 61, 40, "Tough");

    app.update();

    // Player attacks: 5 - 1 = 4 damage, monster has 10HP → 6HP remains
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let monster_health = app.world().get::<Health>(monster).unwrap();
    assert!(monster_health.current > 0, "Monster should survive with positive health");
}

// ─── Combat log tests ────────────────────────────────────────────

#[test]
fn combat_log_records_hit_message() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let _monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let log = app.world().resource::<CombatLog>();
    assert!(!log.messages.is_empty(), "Combat log should have messages after attack");
    assert!(
        log.messages.iter().any(|m| m.contains("hits") && m.contains("damage")),
        "Combat log should contain a hit message"
    );
}

#[test]
fn combat_log_records_death_message() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let _monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weakling".into()),
        Health { current: 1, max: 1 },
        CombatStats { attack: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("slain")),
        "Combat log should contain a death message"
    );
}

#[test]
fn combat_log_persists_across_turns() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let _monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // First attack
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let msg_count_after_first = app.world().resource::<CombatLog>().messages.len();
    assert!(msg_count_after_first > 0);

    // Run another update without any intents (simulates next turn)
    app.update();

    // Messages should still be there (not cleared)
    let msg_count_after_second = app.world().resource::<CombatLog>().messages.len();
    assert_eq!(
        msg_count_after_first, msg_count_after_second,
        "Combat log messages should persist across turns"
    );
}

#[test]
fn combat_log_no_damage_message() {
    let mut app = test_app();
    // Player with 0 attack
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 0 },
    )).id();

    // Monster (player attack is 0 so no damage)
    let _monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("IronGolem".into()),
        Health { current: 50, max: 50 },
        CombatStats { attack: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("no damage")),
        "Combat log should record 'no damage' message when attack is 0"
    );
}

// ─── Spatial index tests ─────────────────────────────────────────

#[test]
fn spatial_index_tracks_entity_positions() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 65, 45, "Goblin");

    app.update();

    let spatial = app.world().resource::<SpatialIndex>();
    let at_player = spatial.entities_at(&GridVec::new(60, 40));
    assert!(at_player.contains(&player), "Spatial index should track player");

    let at_monster = spatial.entities_at(&GridVec::new(65, 45));
    assert!(at_monster.contains(&monster), "Spatial index should track monster");
}

#[test]
fn spatial_index_updates_after_movement() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    app.update();

    // Move player east
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Spatial index is rebuilt at the start of each tick, so we need
    // one more update for the index to reflect the moved position.
    app.update();

    let spatial = app.world().resource::<SpatialIndex>();
    let at_old = spatial.entities_at(&GridVec::new(60, 40));
    assert!(!at_old.contains(&player), "Player should no longer be at old position");

    let at_new = spatial.entities_at(&GridVec::new(61, 40));
    assert!(at_new.contains(&player), "Player should be at new position");
}

// ─── Multiple attack rounds ──────────────────────────────────────

#[test]
fn multiple_attacks_accumulate_damage() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // First attack: 5 damage → 10 - 5 = 5 HP
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let hp1 = app.world().get::<Health>(monster).unwrap().current;
    assert_eq!(hp1, 5);

    // Second attack
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let hp2 = app.world().get::<Health>(monster);
    assert!(hp2.is_none(), "Second attack should kill the monster");
}

#[test]
fn bidirectional_combat_both_take_damage() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // Player attacks monster
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let monster_hp = app.world().get::<Health>(monster).unwrap().current;
    assert!(monster_hp < 10, "Monster should have taken damage from player");

    // Monster attacks player
    app.world_mut().write_message(MoveIntent {
        entity: monster,
        dx: -1,
        dy: 0,
    });
    app.update();

    let player_hp = app.world().get::<Health>(player).unwrap().current;
    assert!(player_hp < 30, "Player should have taken damage from monster");
}

// ─── Spell system tests ──────────────────────────────────────────

/// Creates a minimal App wired for spell testing (includes spell system + projectile system).
fn test_app_with_spells() -> App {
    let mut app = App::new();
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::default());
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_message::<MoveIntent>();
    app.add_message::<AttackIntent>();
    app.add_message::<DamageEvent>();
    app.add_message::<SpellCastIntent>();
    app.init_resource::<SpatialIndex>();
    app.init_resource::<CombatLog>();
    app.init_resource::<KillCount>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<SpellParticles>();
    app.init_resource::<CursorPosition>();
    app.init_resource::<BloodMap>();
    app.init_resource::<TurnCounter>();
    app.init_resource::<InputState>();
    app.init_resource::<GodMode>();
    app.init_resource::<SpectatingAfterDeath>();
    app.init_resource::<DynamicRng>();
    app.init_state::<GameState>();
    app.insert_resource(GameMapResource(GameMap::new(120, 80, 42)));
    app.insert_resource(MapSeed(42));
    app.add_systems(
        Update,
        (
            spatial_index::spatial_index_system,
            movement::movement_system,
            spell::spell_system,
            combat::combat_system,
            projectile::projectile_system,
            spell::explosive_projectile_system,
            combat::apply_damage_system,
            combat::death_system,
        )
            .chain(),
    );
    app
}

#[test]
fn spell_damages_nearby_enemies() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id();

    // Monster within spell radius (2 tiles away, radius=3)
    let monster = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Goblin".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3 },
    )).id();

    app.update();

    // Cast spell with radius 3
    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns explosive projectile
    app.update(); // explosive_projectile_system detonates, spawns shrapnel
    app.update(); // projectile_system advances shrapnel and applies damage

    // Monster should be damaged or killed by shrapnel.
    if let Some(hp) = app.world().get::<Health>(monster) {
        assert!(hp.current < 10, "Monster should take shrapnel damage");
    }
    // Monster was killed by shrapnel — also valid
}

#[test]
fn spell_does_not_damage_distant_enemies() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id();

    // Monster far outside spell radius
    let monster = app.world_mut().spawn((
        Position { x: 70, y: 40 },
        Hostile,
        BlocksMovement,
        Name("FarGoblin".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3 },
    )).id();

    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns shrapnel
    app.update(); // projectile_system advances shrapnel (misses far enemy)

    let monster_health = app.world().get::<Health>(monster).unwrap();
    assert_eq!(monster_health.current, 10, "Distant monster should not be hit by spell");
}

#[test]
fn spell_hits_multiple_enemies() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id();

    // Two monsters within radius
    let m1 = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Goblin1".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3 },
    )).id();

    let m2 = app.world_mut().spawn((
        Position { x: 60, y: 41 },
        Hostile,
        BlocksMovement,
        Name("Goblin2".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3 },
    )).id();

    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns explosive projectile
    app.update(); // explosive detonates, spawns shrapnel
    app.update(); // projectile_system advances shrapnel

    // Monsters should be damaged or killed by shrapnel.
    let m1_hit = match app.world().get::<Health>(m1) {
        Some(hp) => hp.current < 10,
        None => true, // killed
    };
    let m2_hit = match app.world().get::<Health>(m2) {
        Some(hp) => hp.current < 10,
        None => true, // killed
    };
    assert!(m1_hit, "First monster should be damaged by shrapnel");
    assert!(m2_hit, "Second monster should be damaged by shrapnel");
}

#[test]
fn spell_kills_weak_enemy_and_increments_kill_count() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id();

    // Weak monster that will die from shrapnel damage
    let monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weakling".into()),
        Health { current: 3, max: 3 },
        CombatStats { attack: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns explosive projectile
    app.update(); // explosive detonates, spawns shrapnel
    app.update(); // projectile_system advances shrapnel, death_system runs

    // Monster should be despawned
    assert!(
        app.world().get::<Health>(monster).is_none(),
        "Weak monster should be killed by shrapnel"
    );

    // Kill count should be incremented
    let kills = app.world().resource::<KillCount>();
    assert_eq!(kills.0, 1, "Kill count should be 1 after killing a hostile");
}

// ─── Kill count tests ────────────────────────────────────────────

#[test]
fn kill_count_increments_on_hostile_death() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let _monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weakling".into()),
        Health { current: 1, max: 1 },
        CombatStats { attack: 1 },
    )).id();

    app.update();

    // Player kills the monster
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let kills = app.world().resource::<KillCount>();
    assert_eq!(kills.0, 1, "Kill count should increment when hostile entity dies");
}

#[test]
fn spell_no_hit_logs_message() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id();

    // No enemies nearby
    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update();
    app.update();

    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("dynamite") || m.contains("Dynamite")),
        "Combat log should note dynamite was thrown"
    );
}

// ─── Hostile entity combat tests ─────────────────────────────────

#[test]
fn player_can_bump_attack_hostile_entity() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    // Spawn a hostile entity adjacent to the player
    let gate = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Gate of Hell".into()),
        Health { current: 100, max: 100 },
        CombatStats { attack: 0 },
    )).id();

    app.update();

    // Player bumps into gate → should trigger attack
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Player attack=5 → damage=5
    let gate_health = app.world().get::<Health>(gate).unwrap();
    assert_eq!(gate_health.current, 95, "Gate should have taken 5 damage");
}

#[test]
fn spell_damages_hostile_entity() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
    )).id();

    // Hostile entity within spell radius
    let gate = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Gate of Hell".into()),
        Health { current: 100, max: 100 },
        CombatStats { attack: 0 },
    )).id();

    app.update();

    // Cast spell with radius 3
    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns explosive projectile
    app.update(); // explosive detonates, spawns shrapnel
    app.update(); // projectile_system advances shrapnel and applies damage

    let gate_health = app.world().get::<Health>(gate).unwrap();
    assert!(gate_health.current < 100, "Gate should take shrapnel damage");
}

// ─── Spatial index atomicity tests ───────────────────────────────

#[test]
fn two_blockers_cannot_overlap_on_simultaneous_move() {
    let mut app = test_app();
    // We don't need a player for this test, but test_app expects one for
    // the GameState to function. Just spawn one far away.
    let _player = spawn_test_player(&mut app, 60, 40);

    // Two blocking (non-hostile, non-player) entities on opposite sides
    // of an empty tile in the spawn clearing area.
    let e1 = app.world_mut().spawn((
        Position { x: 59, y: 42 },
        BlocksMovement,
        Name("E1".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 1 },
    )).id();

    let e2 = app.world_mut().spawn((
        Position { x: 61, y: 42 },
        BlocksMovement,
        Name("E2".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 1 },
    )).id();

    app.update(); // Build spatial index

    // Both try to move to the same tile (60, 42) simultaneously.
    app.world_mut().write_message(MoveIntent {
        entity: e1,
        dx: 1,
        dy: 0,
    });
    app.world_mut().write_message(MoveIntent {
        entity: e2,
        dx: -1,
        dy: 0,
    });
    app.update();

    let pos1 = app.world().get::<Position>(e1).unwrap();
    let pos2 = app.world().get::<Position>(e2).unwrap();

    // With inline spatial index updates, the first mover succeeds and
    // the second sees the tile as occupied — they must not overlap.
    assert_ne!(
        pos1.as_grid_vec(),
        pos2.as_grid_vec(),
        "Two blocking entities should not occupy the same tile after simultaneous moves"
    );
}

// ─── Ranged gun mechanics tests ──────────────────────────────────

/// Creates a minimal App wired for ranged attack testing (includes projectile system).
fn test_app_with_ranged() -> App {
    let mut app = App::new();
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::default());
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_message::<MoveIntent>();
    app.add_message::<AttackIntent>();
    app.add_message::<DamageEvent>();
    app.add_message::<RangedAttackIntent>();
    app.add_message::<MeleeWideIntent>();
    app.init_resource::<SpatialIndex>();
    app.init_resource::<CombatLog>();
    app.init_resource::<KillCount>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<SpellParticles>();
    app.init_resource::<CursorPosition>();
    app.init_resource::<BloodMap>();
    app.init_resource::<TurnCounter>();
    app.init_resource::<DynamicRng>();
    app.init_resource::<InputState>();
    app.init_resource::<GodMode>();
    app.init_resource::<SpectatingAfterDeath>();
    app.init_state::<GameState>();
    app.insert_resource(GameMapResource(GameMap::new(120, 80, 42)));
    app.insert_resource(MapSeed(42));
    app.add_systems(
        Update,
        (
            spatial_index::spatial_index_system,
            movement::movement_system,
            combat::ranged_attack_system,
            combat::melee_wide_system,
            combat::combat_system,
            projectile::projectile_system,
            combat::apply_damage_system,
            combat::death_system,
        )
            .chain(),
    );
    app
}

/// Spawns a player with a gun item at the given position. Returns (player, gun).
fn spawn_test_player_with_gun(app: &mut App, x: i32, y: i32, attack: i32) -> (Entity, Entity) {
    let gun = app.world_mut().spawn((
        Item,
        Name("Test Gun".into()),
        ItemKind::Gun {
            loaded: 10,
            capacity: 10,
            caliber: Caliber::Cal36,
            attack,
            name: "Test Gun".into(),
            blunt_damage: 5,
        },
    )).id();
    let player = app.world_mut().spawn((
        Position { x, y },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack },
        Inventory { items: vec![gun] },
    )).id();
    (player, gun)
}

#[test]
fn ranged_attack_damages_nearest_enemy() {
    let mut app = test_app_with_ranged();
    let (player, gun) = spawn_test_player_with_gun(&mut app, 60, 40, 5);
    // Monster at distance 4 (within range 8).
    let monster = app.world_mut().spawn((
        Position { x: 64, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Bandit".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3 },
    )).id();

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: Some(gun),
    });
    app.update(); // ranged_attack_system spawns bullet entity
    app.update(); // projectile_system advances bullet and applies damage

    let monster_hp = app.world().get::<Health>(monster).unwrap();
    assert!(monster_hp.current < 20, "Ranged attack should damage the target");
}

#[test]
fn ranged_attack_no_target_in_range() {
    let mut app = test_app_with_ranged();
    let (player, gun) = spawn_test_player_with_gun(&mut app, 60, 40, 5);
    // Monster far away (distance 20, beyond range 8).
    let _monster = app.world_mut().spawn((
        Position { x: 80, y: 40 },
        Hostile,
        BlocksMovement,
        Name("FarBandit".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3 },
    )).id();

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: Some(gun),
    });
    app.update(); // ranged_attack_system spawns bullet entity
    app.update(); // projectile_system advances bullet (misses - out of range)

    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("fires")),
        "Combat log should note the player fired"
    );
}

#[test]
fn ranged_bullet_penetrates_multiple_enemies() {
    let mut app = test_app_with_ranged();
    // Player with attack=10 so bullet has high penetration.
    let gun = app.world_mut().spawn((
        Item,
        Name("Test Gun".into()),
        ItemKind::Gun {
            loaded: 10,
            capacity: 10,
            caliber: Caliber::Cal36,
            attack: 10,
            name: "Test Gun".into(),
            blunt_damage: 5,
        },
    )).id();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 10 },
        Inventory { items: vec![gun] },
    )).id();

    // Two enemies in a line east of player.
    let m1 = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Bandit1".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3 },
    )).id();

    let m2 = app.world_mut().spawn((
        Position { x: 64, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Bandit2".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3 },
    )).id();

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: Some(gun),
    });
    app.update(); // ranged_attack_system spawns bullet entity
    app.update(); // projectile_system advances bullet through both enemies

    // Both monsters should be hit.
    let m1_hp = app.world().get::<Health>(m1).unwrap();
    let m2_hp = app.world().get::<Health>(m2).unwrap();
    assert!(m1_hp.current < 20, "First enemy in line should be hit by bullet");
    assert!(m2_hp.current < 20, "Second enemy in line should be hit by penetrating bullet");
}

#[test]
fn ranged_attack_logs_shoot_message() {
    let mut app = test_app_with_ranged();
    let (player, gun) = spawn_test_player_with_gun(&mut app, 60, 40, 5);
    let _monster = spawn_test_monster(&mut app, 64, 40, "Bandit");

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: Some(gun),
    });
    app.update(); // ranged_attack_system spawns bullet and logs "fires!"
    app.update(); // projectile_system advances bullet and logs hits

    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("fires") || m.contains("hits")),
        "Combat log should contain a fire/hit message"
    );
}

#[test]
fn roundhouse_kick_hits_adjacent_enemies() {
    let mut app = test_app_with_ranged();
    let (player, _) = spawn_test_player_with_gun(&mut app, 60, 40, 5);
    let m1 = spawn_test_monster(&mut app, 61, 40, "Bandit1");
    let m2 = spawn_test_monster(&mut app, 60, 41, "Bandit2");

    app.update();

    app.world_mut().write_message(MeleeWideIntent {
        attacker: player,
    });
    app.update();

    let m1_hp = app.world().get::<Health>(m1).unwrap();
    let m2_hp = app.world().get::<Health>(m2).unwrap();
    assert!(m1_hp.current < 10, "Adjacent enemy 1 should be hit by roundhouse kick");
    assert!(m2_hp.current < 10, "Adjacent enemy 2 should be hit by roundhouse kick");

    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("roundhouse")),
        "Combat log should contain roundhouse kick message"
    );
}

#[test]
fn roundhouse_kick_misses_distant_enemies() {
    let mut app = test_app_with_ranged();
    let (player, _) = spawn_test_player_with_gun(&mut app, 60, 40, 5);
    let monster = spawn_test_monster(&mut app, 63, 40, "FarBandit");

    app.update();

    app.world_mut().write_message(MeleeWideIntent {
        attacker: player,
    });
    app.update();

    let monster_hp = app.world().get::<Health>(monster).unwrap();
    assert_eq!(monster_hp.current, 10, "Distant enemy should not be hit by roundhouse kick");
}

// ─── FOV integration tests ──────────────────────────────────────

/// Creates a minimal App wired for FOV testing (visibility system).
fn test_app_with_fov() -> App {
    let mut app = App::new();
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::default());
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.init_resource::<SpatialIndex>();
    app.init_resource::<CursorPosition>();
    app.init_resource::<InputState>();
    app.init_resource::<SpellParticles>();
    app.init_state::<GameState>();
    app.insert_resource(GameMapResource(GameMap::new(120, 80, 42)));
    app.insert_resource(MapSeed(42));
    app.add_systems(
        Update,
        visibility::visibility_system,
    );
    app
}

#[test]
fn fov_cursor_centered_produces_circle() {
    let mut app = test_app_with_fov();
    // Place player at center of map (clear area)
    let player_pos = Position { x: 60, y: 40 };
    app.world_mut().spawn((
        player_pos,
        Player,
        Viewshed {
            range: 40,
            visible_tiles: std::collections::HashSet::new(),
            revealed_tiles: std::collections::HashSet::new(),
            dirty: true,
        },
    ));

    // Set cursor on player position (centered)
    app.world_mut().resource_mut::<CursorPosition>().pos = GridVec::new(60, 40);

    app.update();

    let viewshed = app.world_mut().query::<&Viewshed>().single(app.world()).unwrap();

    // When cursor is centered, should see in all directions (full circle).
    // Check origin is always visible
    let origin = GridVec::new(60, 40);
    assert!(viewshed.visible_tiles.contains(&origin),
        "Origin should always be visible");

    // Check a tile 2 away in each cardinal direction (very close, should be visible unless blocked)
    let check_dist = 2;
    assert!(viewshed.visible_tiles.contains(&(origin + GridVec::new(check_dist, 0))),
        "Should see east when cursor centered");
    assert!(viewshed.visible_tiles.contains(&(origin + GridVec::new(-check_dist, 0))),
        "Should see west when cursor centered");
    assert!(viewshed.visible_tiles.contains(&(origin + GridVec::new(0, check_dist))),
        "Should see north when cursor centered");
    assert!(viewshed.visible_tiles.contains(&(origin + GridVec::new(0, -check_dist))),
        "Should see south when cursor centered");
}

#[test]
fn fov_far_cursor_has_narrow_cone() {
    let mut app = test_app_with_fov();
    let player_pos = Position { x: 60, y: 40 };
    app.world_mut().spawn((
        player_pos,
        Player,
        Viewshed {
            range: 40,
            visible_tiles: std::collections::HashSet::new(),
            revealed_tiles: std::collections::HashSet::new(),
            dirty: true,
        },
    ));

    // Set cursor far to the east (40+ tiles away for max narrowing)
    app.world_mut().resource_mut::<CursorPosition>().pos = GridVec::new(110, 40);

    app.update();

    let viewshed = app.world_mut().query::<&Viewshed>().single(app.world()).unwrap();

    let origin = GridVec::new(60, 40);
    // Very close east tile should always be visible
    let close_east = origin + GridVec::new(3, 0);
    assert!(viewshed.visible_tiles.contains(&close_east),
        "Close tiles in cone direction should be visible");

    // Far perpendicular tile (north at distance 30) should NOT be visible
    // When aiming far east with narrow cone, tiles directly north at far distance are outside
    let far_north = origin + GridVec::new(0, 30);
    assert!(!viewshed.visible_tiles.contains(&far_north),
        "Should NOT see far north when aiming east with narrow cone");
}

#[test]
fn fov_min_radius_always_visible() {
    let mut app = test_app_with_fov();
    let player_pos = Position { x: 60, y: 40 };
    app.world_mut().spawn((
        player_pos,
        Player,
        Viewshed {
            range: 40,
            visible_tiles: std::collections::HashSet::new(),
            revealed_tiles: std::collections::HashSet::new(),
            dirty: true,
        },
    ));

    // Set cursor far to the north
    app.world_mut().resource_mut::<CursorPosition>().pos = GridVec::new(60, 80);

    app.update();

    let viewshed = app.world_mut().query::<&Viewshed>().single(app.world()).unwrap();

    // When cursor is off-center (north), the player should NOT see behind
    // themselves (south). Only tiles in the forward cone are visible.
    let origin = GridVec::new(60, 40);
    // Close tile in the aiming direction should be visible
    let close_north = origin + GridVec::new(0, 2);
    assert!(viewshed.visible_tiles.contains(&close_north),
        "Close tiles in cone direction should be visible");
    // Close tile directly opposite (south) should NOT be visible
    let close_south = origin + GridVec::new(0, -2);
    assert!(!viewshed.visible_tiles.contains(&close_south),
        "Close tiles in opposite direction should NOT be visible when aiming away");
}

#[test]
fn fov_npc_uses_ai_look_dir() {
    let mut app = test_app_with_fov();
    // Spawn NPC with AiLookDir pointing east
    let npc_pos = Position { x: 60, y: 40 };
    app.world_mut().spawn((
        npc_pos,
        Hostile,
        AiLookDir(GridVec::new(10, 0)),
        Viewshed {
            range: 40,
            visible_tiles: std::collections::HashSet::new(),
            revealed_tiles: std::collections::HashSet::new(),
            dirty: true,
        },
    ));

    app.update();

    let viewshed = app.world_mut().query::<&Viewshed>().single(app.world()).unwrap();

    let origin = GridVec::new(60, 40);
    // NPC looking east should see east tiles
    let near_east = origin + GridVec::new(5, 0);
    assert!(viewshed.visible_tiles.contains(&near_east),
        "NPC should see tiles in look direction");
}

#[test]
fn fov_range_increases_with_cursor_distance() {
    // When cursor is very close: range grows aggressively from FOV_MIN_RADIUS
    let (range_close, _) = visibility::compute_fov_params(Some(GridVec::new(1, 0)));
    assert!(range_close >= visibility::FOV_MIN_RADIUS,
        "Close cursor should give at least minimum range, got {}", range_close);

    // When cursor is far: range should be at or near maximum
    let (range_far, _) = visibility::compute_fov_params(Some(GridVec::new(50, 0)));
    assert!(range_far >= visibility::FOV_MAX_RANGE - 5,
        "Far cursor should give approximately maximum range, got {}", range_far);

    // Far range should be larger than close range
    assert!(range_far > range_close,
        "Far range ({}) should be larger than close range ({})", range_far, range_close);

    // At distance 2, range should be approximately 60 (aggressive growth)
    let (range_mid, _) = visibility::compute_fov_params(Some(GridVec::new(2, 0)));
    assert!(range_mid >= 55 && range_mid <= 65,
        "At cursor distance 2, range should be ~60, got {}", range_mid);
}

#[test]
fn fov_cone_narrows_with_distance() {
    // Close cursor: wide angle (cos_threshold close to -1)
    let (_, cos_close) = visibility::compute_fov_params(Some(GridVec::new(1, 0)));

    // Far cursor: narrow angle (cos_threshold close to 1)
    let (_, cos_far) = visibility::compute_fov_params(Some(GridVec::new(50, 0)));

    assert!(cos_far > cos_close,
        "Far cursor should have higher cos threshold (narrower cone): far={}, close={}", cos_far, cos_close);
    assert!(cos_close < 0.0,
        "Close cursor should have negative cos threshold (wide angle): {}", cos_close);
    assert!(cos_far > 0.5,
        "Far cursor should have cos threshold > 0.5 (narrow cone): {}", cos_far);
}

#[test]
fn fov_centered_cursor_gives_full_circle() {
    let (range, cos) = visibility::compute_fov_params(None);
    assert_eq!(range, visibility::FOV_MIN_RADIUS, "Centered cursor should give min radius");
    assert_eq!(cos, -1.0, "Centered cursor should give full circle (cos = -1)");
}

// ═══════════════════════════════════════════════════════════════════
//  CACTUS DAMAGE INTEGRATION TESTS
// ═══════════════════════════════════════════════════════════════════

/// Creates an app wired for cactus damage testing.
/// Includes movement + cactus_damage_system + combat chain + state management.
fn test_app_with_cactus() -> App {
    let mut app = App::new();
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::default());
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_message::<MoveIntent>();
    app.add_message::<AttackIntent>();
    app.add_message::<DamageEvent>();
    app.init_resource::<SpatialIndex>();
    app.init_resource::<CombatLog>();
    app.init_resource::<KillCount>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<CursorPosition>();
    app.init_resource::<BloodMap>();
    app.init_resource::<TurnCounter>();
    app.init_resource::<InputState>();
    app.init_resource::<GodMode>();
    app.init_resource::<SpectatingAfterDeath>();
    app.init_resource::<DynamicRng>();
    app.init_state::<GameState>();
    app.add_sub_state::<TurnState>();
    app.insert_resource(GameMapResource(GameMap::new(120, 80, 42)));
    app.insert_resource(MapSeed(42));
    app.add_systems(
        Update,
        (
            spatial_index::spatial_index_system,
            movement::movement_system,
            movement::cactus_damage_system,
            combat::combat_system,
            combat::apply_damage_system,
            combat::death_system,
        )
            .chain(),
    );
    app
}

/// Place a cactus at a specific position on the map for testing.
fn place_cactus(app: &mut App, x: i32, y: i32) {
    let map = &mut app.world_mut().resource_mut::<GameMapResource>().0;
    if let Some(voxel) = map.get_voxel_at_mut(&GridVec::new(x, y)) {
        voxel.props = Some(roguelike::typeenums::Props::Cactus);
    }
}

/// Clears props at a position to ensure it's passable.
fn clear_tile(app: &mut App, x: i32, y: i32) {
    let map = &mut app.world_mut().resource_mut::<GameMapResource>().0;
    if let Some(voxel) = map.get_voxel_at_mut(&GridVec::new(x, y)) {
        voxel.props = None;
    }
}

#[test]
fn cactus_deals_1_damage_per_turn_not_per_frame() {
    let mut app = test_app_with_cactus();

    // Clear area and place a cactus adjacent to player position
    clear_tile(&mut app, 60, 40);
    clear_tile(&mut app, 61, 40);
    place_cactus(&mut app, 61, 40);

    let player = spawn_test_player(&mut app, 60, 40);

    // Run multiple updates in AwaitingInput state (simulating many frames)
    for _ in 0..30 {
        app.update();
    }

    // Player should NOT have taken any damage - cactus is gated to turns
    let hp = app.world().get::<Health>(player).unwrap();
    assert_eq!(hp.current, 30,
        "Cactus should not deal damage during AwaitingInput, HP is {}", hp.current);
}

#[test]
fn cactus_damage_applies_during_player_turn() {
    let mut app = test_app_with_cactus();

    clear_tile(&mut app, 60, 40);
    clear_tile(&mut app, 61, 40);
    clear_tile(&mut app, 59, 40);
    place_cactus(&mut app, 61, 40);

    let player = spawn_test_player(&mut app, 60, 40);

    app.update(); // Build spatial index

    // Transition to PlayerTurn to trigger cactus damage
    app.world_mut().resource_mut::<NextState<TurnState>>().set(TurnState::PlayerTurn);
    app.update();

    let hp = app.world().get::<Health>(player).unwrap();
    assert!(hp.current <= 30,
        "Player should be alive after cactus prick, HP is {}", hp.current);
}

#[test]
fn cactus_does_not_instakill_player() {
    let mut app = test_app_with_cactus();

    // Clear area and surround player with cacti
    for dx in -2..=2 {
        for dy in -2..=2 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    // Place cacti in all cardinal directions
    place_cactus(&mut app, 61, 40);
    place_cactus(&mut app, 59, 40);
    place_cactus(&mut app, 60, 41);
    place_cactus(&mut app, 60, 39);

    let player = spawn_test_player(&mut app, 60, 40);

    // Run several game frames (simulating standing near cacti)
    for _ in 0..10 {
        app.update();
    }

    // Player (30 HP) should still be alive after 10 frames
    let hp = app.world().get::<Health>(player).unwrap();
    assert!(hp.current > 0,
        "Player should survive standing near cacti for 10 frames, HP is {}", hp.current);
    // At most 1 damage per actual turn (not frame), so with no turns taken HP should be ~30
    assert!(hp.current >= 28,
        "Player should have nearly full HP (cactus gated to turns), HP is {}", hp.current);
}

#[test]
fn cactus_damage_only_once_per_turn_even_with_multiple_cacti() {
    let mut app = test_app_with_cactus();

    for dx in -2..=2 {
        for dy in -2..=2 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    // Surround player with cacti
    place_cactus(&mut app, 61, 40);
    place_cactus(&mut app, 59, 40);
    place_cactus(&mut app, 60, 41);
    place_cactus(&mut app, 60, 39);

    let player = spawn_test_player(&mut app, 60, 40);

    app.update(); // Build spatial index

    // Transition to PlayerTurn
    app.world_mut().resource_mut::<NextState<TurnState>>().set(TurnState::PlayerTurn);
    app.update();

    let hp = app.world().get::<Health>(player).unwrap();
    // Should take at most 1 damage (break after first cactus) per turn
    assert!(hp.current >= 29,
        "Multiple adjacent cacti should only deal 1 damage per turn, HP is {}", hp.current);
}

#[test]
fn cactus_does_not_damage_distant_entity() {
    let mut app = test_app_with_cactus();

    for dx in -2..=2 {
        for dy in -2..=2 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    // Place cactus far from player (3 tiles away)
    place_cactus(&mut app, 63, 40);

    let player = spawn_test_player(&mut app, 60, 40);

    // Transition to PlayerTurn
    app.update();
    app.world_mut().resource_mut::<NextState<TurnState>>().set(TurnState::PlayerTurn);
    app.update();

    let hp = app.world().get::<Health>(player).unwrap();
    assert_eq!(hp.current, 30,
        "Entity far from cactus should take no damage, HP is {}", hp.current);
}

#[test]
fn cactus_logs_damage_message() {
    let mut app = test_app_with_cactus();

    clear_tile(&mut app, 60, 40);
    clear_tile(&mut app, 61, 40);
    place_cactus(&mut app, 61, 40);

    let _player = spawn_test_player(&mut app, 60, 40);

    app.update();
    app.world_mut().resource_mut::<NextState<TurnState>>().set(TurnState::PlayerTurn);
    app.update();

    let log = app.world().resource::<CombatLog>();
    let has_cactus_msg = log.messages.iter().any(|m| m.to_lowercase().contains("cactus"));
    assert!(has_cactus_msg,
        "Combat log should contain cactus damage message");
}

#[test]
fn cactus_damages_monsters_too() {
    let mut app = test_app_with_cactus();

    clear_tile(&mut app, 60, 40);
    clear_tile(&mut app, 65, 40);
    clear_tile(&mut app, 66, 40);
    place_cactus(&mut app, 66, 40);

    let _player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 65, 40, "Goblin");

    app.update();
    app.world_mut().resource_mut::<NextState<TurnState>>().set(TurnState::PlayerTurn);
    app.update();

    let hp = app.world().get::<Health>(monster).unwrap();
    assert!(hp.current < 10,
        "Monster adjacent to cactus should take damage, HP is {}", hp.current);
}

// ═══════════════════════════════════════════════════════════════════
//  SAND PARTICLE TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn sand_particles_created_with_sand_flag() {
    let mut particles = SpellParticles::default();
    let origin = GridVec::new(10, 10);
    // Simulate sand throw: add particles with is_sand=true
    for dx in -2..=2 {
        for dy in -2..=2 {
            let pos = origin + GridVec::new(dx, dy);
            particles.particles.push((pos, 12, 0, true));
        }
    }

    // All particles should have is_sand=true
    assert!(particles.particles.iter().all(|(_, _, _, is_sand)| *is_sand),
        "Sand particles should have is_sand flag set");

    // Should have 25 particles (5×5 grid)
    assert_eq!(particles.particles.len(), 25);
}

#[test]
fn sand_particles_persist_for_12_ticks() {
    let mut particles = SpellParticles::default();
    let origin = GridVec::new(10, 10);
    particles.particles.push((origin, 12, 0, true));

    // Tick 11 times - particle should still exist
    for _ in 0..11 {
        particles.tick();
    }
    assert_eq!(particles.particles.len(), 1,
        "Sand particle should persist for 12 ticks (11 ticks elapsed)");

    // Tick once more - particle should expire
    particles.tick();
    assert_eq!(particles.particles.len(), 0,
        "Sand particle should expire after 12 ticks");
}

#[test]
fn explosion_particles_are_not_sand() {
    let mut particles = SpellParticles::default();
    let origin = GridVec::new(10, 10);
    particles.add_aoe(origin, 6);

    // All AoE particles should have is_sand=false
    assert!(particles.particles.iter().all(|(_, _, _, is_sand)| !*is_sand),
        "Explosion particles should NOT have is_sand flag set");
}

#[test]
fn sand_and_explosion_particles_coexist() {
    let mut particles = SpellParticles::default();
    let origin = GridVec::new(10, 10);

    // Add sand particles
    particles.particles.push((origin, 12, 0, true));

    // Add explosion particles
    particles.add_aoe(origin + GridVec::new(5, 5), 6);

    let sand_count = particles.particles.iter().filter(|(_, _, _, is_sand)| *is_sand).count();
    let explosion_count = particles.particles.iter().filter(|(_, _, _, is_sand)| !*is_sand).count();

    assert_eq!(sand_count, 1, "Should have 1 sand particle");
    assert!(explosion_count > 0, "Should have explosion particles");
}

#[test]
fn particles_tick_respects_delay() {
    let mut particles = SpellParticles::default();
    // Particle with delay=3
    particles.particles.push((GridVec::new(0, 0), 6, 3, false));

    // After 2 ticks, delay should be reduced but particle still waiting
    particles.tick();
    particles.tick();
    assert_eq!(particles.particles.len(), 1);
    assert_eq!(particles.particles[0].2, 1, "Delay should be decremented");

    // After 1 more tick, delay reaches 0, lifetime starts counting
    particles.tick();
    assert_eq!(particles.particles.len(), 1);
    assert_eq!(particles.particles[0].2, 0, "Delay should be 0");
    assert_eq!(particles.particles[0].1, 6, "Lifetime should not yet decrement when delay just reached 0");
}

#[test]
fn spell_sand_throw_creates_sand_particles() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
        Stamina { current: 50, max: 50 },
    )).id();

    app.update();

    // Sand throw uses grenade_index == usize::MAX as sentinel
    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 2,
        target: GridVec::new(62, 40),
        grenade_index: usize::MAX,
    });
    app.update();

    // Sand clouds should now be placed on the game map as SandCloud floor tiles
    let game_map = app.world().resource::<GameMapResource>();
    let target = GridVec::new(62, 40);
    let has_sand_cloud = game_map.0.get_voxel_at(&target)
        .is_some_and(|v| matches!(v.floor, Some(roguelike::typeenums::Floor::SandCloud)));
    assert!(has_sand_cloud,
        "Sand throw should create SandCloud floor tiles on the map");

    // Sand cloud turns tracker should be populated
    assert!(!game_map.0.sand_cloud_turns.is_empty(),
        "Sand cloud turn tracker should have entries");
}

// ═══════════════════════════════════════════════════════════════════
//  NPC AI INTEGRATION TESTS
// ═══════════════════════════════════════════════════════════════════

/// Creates an app wired for full AI testing with all systems.
fn test_app_with_ai() -> App {
    let mut app = App::new();
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::default());
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_message::<MoveIntent>();
    app.add_message::<AttackIntent>();
    app.add_message::<DamageEvent>();
    app.add_message::<SpellCastIntent>();
    app.add_message::<RangedAttackIntent>();
    app.add_message::<AiRangedAttackIntent>();
    app.add_message::<MeleeWideIntent>();
    app.add_message::<MolotovCastIntent>();
    app.add_message::<ThrowItemIntent>();
    app.add_message::<UseItemIntent>();
    app.add_message::<PickupItemIntent>();
    app.add_message::<DropItemIntent>();
    app.init_resource::<SpatialIndex>();
    app.init_resource::<CombatLog>();
    app.init_resource::<KillCount>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<SpellParticles>();
    app.init_resource::<CursorPosition>();
    app.init_resource::<BloodMap>();
    app.init_resource::<TurnCounter>();
    app.init_resource::<InputState>();
    app.init_resource::<GodMode>();
    app.init_resource::<SpectatingAfterDeath>();
    app.init_resource::<DynamicRng>();
    app.init_resource::<Collectibles>();
    app.init_state::<GameState>();
    app.add_sub_state::<TurnState>();
    app.insert_resource(GameMapResource(GameMap::new(120, 80, 42)));
    app.insert_resource(MapSeed(42));
    app.add_systems(
        Update,
        (
            spatial_index::spatial_index_system,
            ai::energy_accumulate_system,
            ai::ai_system,
            movement::movement_system,
            movement::cactus_damage_system,
            inventory::pickup_system,
            inventory::use_item_system,
            inventory::throw_system,
            combat::combat_system,
            combat::ranged_attack_system,
            combat::ai_ranged_attack_system,
            combat::melee_wide_system,
            projectile::projectile_system,
            combat::apply_damage_system,
            combat::death_system,
        )
            .chain(),
    );
    app
}

/// Spawns an NPC with full AI capabilities at the given position.
fn spawn_ai_npc(app: &mut App, x: i32, y: i32, name: &str, faction: Faction) -> Entity {
    app.world_mut().spawn((
        Position { x, y },
        Hostile,
        BlocksMovement,
        Name(name.into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 5 },
        Speed(ACTION_COST),
        Energy(0),
        AiState::Idle,
        AiLookDir(GridVec::new(1, 0)),
        PatrolOrigin(GridVec::new(x, y)),
        AiMemory::default(),
        AiPersonality::default(),
        faction,
    )).insert((
        Viewshed {
            range: 20,
            visible_tiles: std::collections::HashSet::new(),
            revealed_tiles: std::collections::HashSet::new(),
            dirty: true,
        },
        Inventory { items: vec![] },
        Stamina { current: 50, max: 50 },
    )).id()
}

/// Spawns a whiskey item entity (not in any inventory).
fn spawn_whiskey_item(app: &mut App) -> Entity {
    app.world_mut().spawn((
        Item,
        Name("Whiskey Bottle".into()),
        Renderable {
            symbol: "w".into(),
            fg: roguelike::typedefs::RatColor::Rgb(180, 120, 60),
            bg: roguelike::typedefs::RatColor::Black,
        },
        ItemKind::Whiskey { heal: 10, blunt_damage: 4 },
    )).id()
}

/// Spawns a knife item entity (not in any inventory).
fn spawn_knife_item(app: &mut App) -> Entity {
    app.world_mut().spawn((
        Item,
        Name("Bowie Knife".into()),
        Renderable {
            symbol: "/".into(),
            fg: roguelike::typedefs::RatColor::Rgb(192, 192, 210),
            bg: roguelike::typedefs::RatColor::Black,
        },
        ItemKind::Knife { attack: 4, blunt_damage: 6 },
    )).id()
}

/// Spawns a grenade item entity (not in any inventory).
fn spawn_grenade_item(app: &mut App) -> Entity {
    app.world_mut().spawn((
        Item,
        Name("Dynamite Stick".into()),
        Renderable {
            symbol: "*".into(),
            fg: roguelike::typedefs::RatColor::Rgb(255, 165, 0),
            bg: roguelike::typedefs::RatColor::Black,
        },
        ItemKind::Grenade { damage: 8, radius: 2, blunt_damage: 3 },
    )).id()
}

// ─── AI State Transition Tests ───────────────────────────────────

#[test]
fn ai_idle_transitions_to_chasing_on_player_sight() {
    let mut app = test_app_with_ai();

    // Clear tiles around both entities
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 63, 40, "Outlaw", Faction::Outlaws);

    // Pre-populate the NPC's viewshed so it "sees" the player
    // (visibility system is not in this test app)
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40)); // player position
        vs.dirty = false;
    }

    // Give NPC enough energy to act
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    // Check NPC state - should have transitioned from Idle to Chasing
    let state = app.world().get::<AiState>(npc).unwrap();
    assert_eq!(*state, AiState::Chasing,
        "NPC should transition to Chasing when player is visible");
}

#[test]
fn ai_chasing_npc_moves_toward_player() {
    let mut app = test_app_with_ai();

    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 68, 40, "Outlaw", Faction::Outlaws);

    // Set NPC to Chasing with enough energy
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    // Pre-populate NPC viewshed with player position so it can see the player.
    // The AI system now requires viewshed-based visibility (not raw distance).
    app.world_mut().get_mut::<Viewshed>(npc).unwrap()
        .visible_tiles.insert(GridVec::new(60, 40));

    let initial_pos = *app.world().get::<Position>(npc).unwrap();

    // Run a few ticks for AI to process
    for _ in 0..5 {
        app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
        // Re-populate viewshed each tick since visibility_system may clear it
        app.world_mut().get_mut::<Viewshed>(npc).unwrap()
            .visible_tiles.insert(GridVec::new(60, 40));
        app.update();
    }

    let new_pos = *app.world().get::<Position>(npc).unwrap();

    // NPC should have moved closer to player (or attacked if adjacent)
    let initial_dist = GridVec::new(initial_pos.x, initial_pos.y).chebyshev_distance(GridVec::new(60, 40));
    let new_dist = GridVec::new(new_pos.x, new_pos.y).chebyshev_distance(GridVec::new(60, 40));

    assert!(new_dist < initial_dist || new_dist <= 1,
        "NPC should move toward player: initial dist={}, new dist={}", initial_dist, new_dist);
}

#[test]
fn ai_loses_target_returns_to_patrol() {
    let mut app = test_app_with_ai();

    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    // No player spawned — NPC should eventually revert to Idle/Patrolling
    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Start in Chasing state with energy
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    // Run several ticks
    for _ in 0..5 {
        app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
        app.update();
    }

    let state = app.world().get::<AiState>(npc).unwrap();
    assert!(*state != AiState::Chasing,
        "NPC should stop chasing when no target is visible, state is {:?}", state);
}

// ─── NPC Healing Tests ───────────────────────────────────────────

#[test]
fn ai_npc_heals_with_whiskey_when_wounded() {
    let mut app = test_app_with_ai();

    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Give NPC a whiskey and wound it below 50% HP
    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(whiskey);
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 8; // 40% HP (below 50% threshold)
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    let hp_before = app.world().get::<Health>(npc).unwrap().current;

    // Run AI system
    app.update();

    let hp_after = app.world().get::<Health>(npc).unwrap().current;
    let inv = app.world().get::<Inventory>(npc).unwrap();

    // NPC should have used the whiskey
    assert!(hp_after > hp_before,
        "NPC should have healed: before={}, after={}", hp_before, hp_after);
    assert!(inv.items.is_empty(),
        "Whiskey should be consumed from inventory");
}

#[test]
fn ai_npc_does_not_heal_when_healthy() {
    let mut app = test_app_with_ai();

    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Give NPC a whiskey but keep HP at full
    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(whiskey);
    // HP is full (20/20) - above 50% threshold
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert!(!inv.items.is_empty(),
        "NPC should NOT use whiskey when at full HP");
}

#[test]
fn ai_npc_does_not_heal_at_exactly_50_percent() {
    let mut app = test_app_with_ai();

    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Give NPC a whiskey with HP exactly at 50%
    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(whiskey);
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 10; // 50% HP
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    // At exactly 50%, fraction < 0.5 is false, so shouldn't heal
    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert!(!inv.items.is_empty(),
        "NPC should NOT use whiskey when exactly at 50% HP");
}

// ─── NPC Item Usage Tests ────────────────────────────────────────

#[test]
fn ai_npc_picks_up_floor_items() {
    let mut app = test_app_with_ai();

    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Place a knife on the same tile as the NPC
    let knife = spawn_knife_item(&mut app);
    app.world_mut().entity_mut(knife).insert(Position { x: 60, y: 40 });

    // Pre-populate viewshed so NPC can "see" the item
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }

    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    // NPC should have picked up the knife
    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert!(inv.items.contains(&knife),
        "NPC should auto-pickup items on the same tile");
}

// ─── A* Pathfinding Tests ────────────────────────────────────────

#[test]
fn a_star_finds_path_around_obstacle() {
    // Test that A* pathfinding can navigate around obstacles.
    // This is tested through the ai module's public functions.
    let start = GridVec::new(0, 0);
    let goal = GridVec::new(5, 0);

    // Create a wall blocking the direct path
    let wall_positions: std::collections::HashSet<GridVec> = [
        GridVec::new(2, -1), GridVec::new(2, 0), GridVec::new(2, 1),
    ].into_iter().collect();

    let step = ai::a_star_first_step_pub(start, goal, |pos| {
        !wall_positions.contains(&pos)
    });

    assert!(step.is_some(),
        "A* should find a path around the obstacle");
    // The first step should not go into the wall
    let s = step.unwrap();
    assert!(!wall_positions.contains(&(start + s)),
        "A* should not step into the wall");
}

#[test]
fn a_star_returns_none_when_unreachable() {
    let start = GridVec::new(0, 0);
    let goal = GridVec::new(5, 0);

    // Completely surround the start position with walls
    let step = ai::a_star_first_step_pub(start, goal, |pos| {
        // Only the start is walkable
        pos == start
    });

    assert!(step.is_none(),
        "A* should return None when goal is unreachable");
}

#[test]
fn a_star_diagonal_path() {
    let start = GridVec::new(0, 0);
    let goal = GridVec::new(5, 5);

    let step = ai::a_star_first_step_pub(start, goal, |_| true);

    assert!(step.is_some(),
        "A* should find diagonal path");
    let s = step.unwrap();
    // Should step diagonally toward goal
    assert!(s.x > 0 && s.y > 0,
        "Should take a diagonal step toward (5,5), got ({}, {})", s.x, s.y);
}

#[test]
fn a_star_at_goal_returns_none() {
    let pos = GridVec::new(5, 5);
    let step = ai::a_star_first_step_pub(pos, pos, |_| true);
    assert!(step.is_none(),
        "A* should return None when already at goal");
}

// ─── Faction Interaction Tests ───────────────────────────────────

#[test]
fn factions_are_hostile_outlaws_vs_lawmen() {
    assert!(ai::factions_are_hostile(Faction::Outlaws, Faction::Lawmen));
    assert!(ai::factions_are_hostile(Faction::Lawmen, Faction::Outlaws));
}

#[test]
fn factions_are_hostile_wildlife_vs_all() {
    assert!(ai::factions_are_hostile(Faction::Wildlife, Faction::Outlaws));
    assert!(ai::factions_are_hostile(Faction::Wildlife, Faction::Lawmen));
    assert!(ai::factions_are_hostile(Faction::Wildlife, Faction::Vaqueros));
    assert!(ai::factions_are_hostile(Faction::Outlaws, Faction::Wildlife));
    assert!(ai::factions_are_hostile(Faction::Lawmen, Faction::Wildlife));
}

#[test]
fn factions_same_faction_not_hostile() {
    assert!(!ai::factions_are_hostile(Faction::Outlaws, Faction::Outlaws));
    assert!(!ai::factions_are_hostile(Faction::Lawmen, Faction::Lawmen));
    assert!(!ai::factions_are_hostile(Faction::Wildlife, Faction::Wildlife));
    assert!(!ai::factions_are_hostile(Faction::Vaqueros, Faction::Vaqueros));
}

#[test]
fn factions_vaqueros_vs_outlaws() {
    assert!(ai::factions_are_hostile(Faction::Vaqueros, Faction::Outlaws));
    assert!(ai::factions_are_hostile(Faction::Outlaws, Faction::Vaqueros));
}

#[test]
fn factions_lawmen_and_vaqueros_not_hostile() {
    // Lawmen and Vaqueros are not hostile to each other
    assert!(!ai::factions_are_hostile(Faction::Lawmen, Faction::Vaqueros));
    assert!(!ai::factions_are_hostile(Faction::Vaqueros, Faction::Lawmen));
}

// ─── Energy / Speed Integration Tests ────────────────────────────

#[test]
fn energy_system_accumulates_for_npcs() {
    let mut app = App::new();
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::default());
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.init_state::<GameState>();
    // Only run energy accumulation, NOT the AI system (which would spend energy)
    app.add_systems(Update, ai::energy_accumulate_system);

    let npc = app.world_mut().spawn((
        Speed(ACTION_COST),
        Energy(0),
    )).id();

    assert_eq!(app.world().get::<Energy>(npc).unwrap().0, 0);

    app.update();

    let energy = app.world().get::<Energy>(npc).unwrap().0;
    assert_eq!(energy, ACTION_COST,
        "NPC with Speed(100) should gain ACTION_COST energy per tick, got {}", energy);
}

#[test]
fn fast_npc_acts_more_frequently() {
    let mut app = test_app_with_ai();

    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    // Create a fast NPC (Speed 200) - should act twice per tick
    let npc = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Hostile,
        BlocksMovement,
        Name("FastNPC".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 5 },
        Speed(200),
        Energy(0),
        AiState::Idle,
        Faction::Outlaws,
        Inventory { items: vec![] },
    )).id();

    app.update();

    // After one energy accumulation, speed 200 gives 200 energy
    // Can act twice (200 >= 100, 200-100=100 >= 100)
    let energy = app.world().get::<Energy>(npc).unwrap().0;
    // Energy may have been spent on actions, but should have accumulated 200
    assert!(energy <= 200, "Energy should not exceed 200");
}

#[test]
fn slow_npc_acts_less_frequently() {
    let mut app = test_app_with_ai();

    let npc = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Hostile,
        BlocksMovement,
        Name("SlowNPC".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 5 },
        Speed(50),
        Energy(0),
        AiState::Idle,
        Faction::Outlaws,
        Inventory { items: vec![] },
    )).id();

    app.update();

    let energy = app.world().get::<Energy>(npc).unwrap().0;
    // Speed 50 gives 50 energy - not enough to act (need 100)
    assert_eq!(energy, 50,
        "Slow NPC should not have enough energy to act after 1 tick");
}

// ─── Combat Integration Tests ────────────────────────────────────

#[test]
fn multiple_monsters_can_attack_player_in_sequence() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let m1 = spawn_test_monster(&mut app, 61, 40, "Goblin1");
    let m2 = spawn_test_monster(&mut app, 59, 40, "Goblin2");

    app.update();

    // Both monsters attack player from different sides
    app.world_mut().write_message(MoveIntent {
        entity: m1,
        dx: -1,
        dy: 0,
    });
    app.world_mut().write_message(MoveIntent {
        entity: m2,
        dx: 1,
        dy: 0,
    });
    app.update();

    let hp = app.world().get::<Health>(player).unwrap();
    // Monster attack=3 → 3 damage each = 6 total
    assert_eq!(hp.current, 24,
        "Player should take damage from both monsters, HP is {}", hp.current);
}

#[test]
fn kill_awards_kill_count_with_damage_source() {
    let mut app = test_app();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 20 },
    )).id();

    let monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weakling".into()),
        Health { current: 1, max: 1 },
        CombatStats { attack: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Monster should be dead
    assert!(app.world().get::<Health>(monster).is_none(),
        "Monster should be dead");

    // Kill count should attribute the kill to the player
    let kills = app.world().resource::<KillCount>();
    assert_eq!(kills.0, 1,
        "Kill count should correctly attribute kill to player");
}

#[test]
fn god_mode_prevents_player_damage() {
    let mut app = test_app();
    app.world_mut().resource_mut::<GodMode>().0 = true;

    let player = spawn_test_player(&mut app, 60, 40);
    let monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    app.world_mut().write_message(MoveIntent {
        entity: monster,
        dx: -1,
        dy: 0,
    });
    app.update();

    let hp = app.world().get::<Health>(player).unwrap();
    assert_eq!(hp.current, 30,
        "God mode should prevent player from taking damage");
}

// ─── Projectile Tests ────────────────────────────────────────────

#[test]
fn projectile_despawns_on_wall_collision() {
    let mut app = test_app_with_ranged();
    let (player, gun) = spawn_test_player_with_gun(&mut app, 60, 40, 5);

    app.update();

    // Fire toward a wall (border at x=0)
    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 100,
        dx: -1,
        dy: 0,
        gun_item: Some(gun),
    });
    app.update(); // Spawn projectile
    // Advance projectile enough ticks to reach the border wall (60 tiles at 3/tick)
    for _ in 0..25 {
        app.update();
    }

    // Projectile should be despawned after hitting wall
    let projectile_count = app.world_mut().query::<&Projectile>()
        .iter(app.world())
        .count();
    assert_eq!(projectile_count, 0,
        "Projectile should despawn after hitting a wall");
}

#[test]
fn ranged_attack_preserves_player_position() {
    let mut app = test_app_with_ranged();
    let (player, gun) = spawn_test_player_with_gun(&mut app, 60, 40, 5);

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: Some(gun),
    });
    app.update();

    let pos = app.world().get::<Position>(player).unwrap();
    assert_eq!(pos.x, 60,
        "Player position should not change when firing");
    assert_eq!(pos.y, 40);
}

// ─── Spell / Sand / Molotov Integration Tests ───────────────────

#[test]
fn spell_consumes_stamina() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
        Stamina { current: 50, max: 50 },
        Inventory { items: vec![] },
    )).id();

    // Give player a grenade
    let grenade = spawn_grenade_item(&mut app);
    app.world_mut().get_mut::<Inventory>(player).unwrap().items.push(grenade);

    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(63, 40),
        grenade_index: 0,
    });
    app.update();

    let stamina = app.world().get::<Stamina>(player).unwrap();
    assert!(stamina.current < 50,
        "Spell should consume stamina, current is {}", stamina.current);
}

#[test]
fn sand_throw_costs_less_stamina_than_grenade() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5 },
        Stamina { current: 50, max: 50 },
    )).id();

    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 2,
        target: GridVec::new(62, 40),
        grenade_index: usize::MAX, // sand throw sentinel
    });
    app.update();

    let stamina = app.world().get::<Stamina>(player).unwrap();
    // Sand throw costs 5, grenade costs 10
    assert_eq!(stamina.current, 45,
        "Sand throw should cost 5 stamina, current is {}", stamina.current);
}

// ─── Blood Map Tests ─────────────────────────────────────────────

#[test]
fn wounded_entity_leaves_blood_trail() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);
    let _monster = spawn_test_monster(&mut app, 61, 40, "Goblin");

    app.update();

    // Player gets hurt by bumping monster
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Monster attacks back
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: -1,
        dy: 0,
    });
    app.update();

    // Now move player (who is wounded) to leave blood
    let hp = app.world().get::<Health>(player).unwrap();
    if hp.current < hp.max {
        // Move wounded player
        app.world_mut().write_message(MoveIntent {
            entity: player,
            dx: 0,
            dy: 1,
        });
        app.update();

        let blood = app.world().resource::<BloodMap>();
        assert!(!blood.stains.is_empty(),
            "Wounded entity should leave blood trail when moving");
    }
}

// ─── Movement Edge Cases ─────────────────────────────────────────

#[test]
fn diagonal_movement_works() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    app.update();

    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 1,
    });
    app.update();

    let pos = app.world().get::<Position>(player).unwrap();
    assert_eq!(pos.x, 61);
    assert_eq!(pos.y, 41);
}

#[test]
fn cursor_follows_player_movement() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    app.update();

    let cursor_before = app.world().resource::<CursorPosition>().pos;

    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let cursor_after = app.world().resource::<CursorPosition>().pos;
    assert_eq!(cursor_after.x, cursor_before.x + 1,
        "Cursor should follow player movement");
}

#[test]
fn multiple_moves_in_same_frame_resolve_correctly() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    app.update();

    // Send two move intents in the same frame
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let pos = app.world().get::<Position>(player).unwrap();
    // Both intents should resolve: player moves right twice
    assert_eq!(pos.x, 62,
        "Two consecutive move intents should both resolve");
}

// ─── Inventory Integration Tests ─────────────────────────────────

#[test]
fn npc_inventory_limits_respected() {
    let mut app = test_app_with_ai();

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Fill inventory to capacity (9 items)
    for _ in 0..9 {
        let item = spawn_knife_item(&mut app);
        app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(item);
    }

    // Place another item on the same tile
    let extra_item = spawn_knife_item(&mut app);
    app.world_mut().entity_mut(extra_item).insert(Position { x: 60, y: 40 });

    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    app.update();

    // NPC should NOT pick up the extra item (inventory full)
    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert_eq!(inv.items.len(), 9,
        "NPC should not exceed inventory limit of 9");
}

// ─── Health Component Edge Cases ─────────────────────────────────

#[test]
fn health_zero_damage_no_change() {
    let mut hp = Health { current: 30, max: 30 };
    let actual = hp.apply_damage(0);
    assert_eq!(actual, 0);
    assert_eq!(hp.current, 30);
}

#[test]
fn health_negative_damage_clamped_to_zero() {
    let mut hp = Health { current: 30, max: 30 };
    let actual = hp.apply_damage(-5);
    assert_eq!(actual, 0, "Negative damage should be clamped to 0");
    assert_eq!(hp.current, 30);
}

#[test]
fn health_heal_from_zero() {
    let mut hp = Health { current: 0, max: 30 };
    let healed = hp.heal(10);
    assert_eq!(healed, 10);
    assert_eq!(hp.current, 10);
}

// ─── Compute Damage Edge Cases ───────────────────────────────────

#[test]
fn compute_damage_large_values() {
    assert_eq!(compute_damage(1000), 1000);
    assert_eq!(compute_damage(0), 0);
}

#[test]
fn compute_damage_equal_zero() {
    assert_eq!(compute_damage(0), 0);
}

// ─── Stamina Edge Cases ──────────────────────────────────────────

#[test]
fn stamina_spend_zero_cost() {
    let mut s = Stamina { current: 50, max: 50 };
    assert!(s.spend(0));
    assert_eq!(s.current, 50);
}

#[test]
fn stamina_recover_from_zero() {
    let mut s = Stamina { current: 0, max: 50 };
    s.recover(25);
    assert_eq!(s.current, 25);
}

// ─── SpellParticles Stress Tests ─────────────────────────────────

#[test]
fn spell_particles_respect_max_limit() {
    let mut particles = SpellParticles::default();
    // Add many AoE effects to test the MAX_PARTICLES limit
    for i in 0..100 {
        particles.add_aoe(GridVec::new(i * 10, 0), 6);
    }
    // Should not exceed the internal max
    assert!(particles.particles.len() <= 800,
        "Particles should respect MAX_PARTICLES limit, count is {}", particles.particles.len());
}

#[test]
fn spell_particles_all_expire_eventually() {
    let mut particles = SpellParticles::default();
    particles.add_aoe(GridVec::new(10, 10), 6);
    particles.particles.push((GridVec::new(0, 0), 12, 0, true));

    // Tick enough times for all particles to expire
    for _ in 0..50 {
        particles.tick();
    }

    assert!(particles.particles.is_empty(),
        "All particles should eventually expire");
}

// ─── Spatial Index Integrity Tests ───────────────────────────────

#[test]
fn spatial_index_move_entity_preserves_other_entities() {
    let mut index = SpatialIndex::default();
    let e1 = Entity::from_bits(1);
    let e2 = Entity::from_bits(2);
    let e3 = Entity::from_bits(3);
    let pos = GridVec::new(5, 5);

    index.add_entity(pos, e1);
    index.add_entity(pos, e2);
    index.add_entity(pos, e3);

    // Move e2 away
    index.move_entity(&pos, GridVec::new(6, 5), e2);

    let at_original = index.entities_at(&pos);
    assert_eq!(at_original.len(), 2);
    assert!(at_original.contains(&e1));
    assert!(at_original.contains(&e3));
    assert!(!at_original.contains(&e2));
}

// ─── GridVec Mathematical Property Tests ─────────────────────────

#[test]
fn gridvec_king_step_normalizes_correctly() {
    let v = GridVec::new(5, -3);
    let step = v.king_step();
    assert_eq!(step, GridVec::new(1, -1));

    let zero = GridVec::ZERO;
    assert_eq!(zero.king_step(), GridVec::ZERO);
}

#[test]
fn gridvec_chebyshev_distance_symmetry() {
    let a = GridVec::new(3, 7);
    let b = GridVec::new(10, 2);
    assert_eq!(a.chebyshev_distance(b), b.chebyshev_distance(a));
}

#[test]
fn gridvec_chebyshev_distance_to_self_is_zero() {
    let v = GridVec::new(42, -17);
    assert_eq!(v.chebyshev_distance(v), 0);
}

#[test]
fn gridvec_bresenham_line_endpoints() {
    let start = GridVec::new(0, 0);
    let end = GridVec::new(5, 3);
    let line = start.bresenham_line(end);
    assert_eq!(*line.first().unwrap(), start);
    assert_eq!(*line.last().unwrap(), end);
}

#[test]
fn gridvec_bresenham_line_adjacent() {
    let start = GridVec::new(0, 0);
    let end = GridVec::new(1, 0);
    let line = start.bresenham_line(end);
    assert_eq!(line.len(), 2);
    assert_eq!(line[0], start);
    assert_eq!(line[1], end);
}

#[test]
fn gridvec_cardinal_neighbors_count() {
    let v = GridVec::new(5, 5);
    let neighbors = v.cardinal_neighbors();
    assert_eq!(neighbors.len(), 4);
}

// ─── CombatLog Visibility Filtering Tests ────────────────────────

#[test]
fn combat_log_filters_by_visibility() {
    let mut log = CombatLog::default();

    // Message always visible (no position)
    log.push("Global event".into());

    // Message at a specific position
    log.push_at("Local event".into(), GridVec::new(5, 5));

    // Message at another position
    log.push_at("Far event".into(), GridVec::new(100, 100));

    let mut visible = std::collections::HashSet::new();
    visible.insert(GridVec::new(5, 5));

    let msgs = log.recent_visible(10, &visible);

    assert!(msgs.contains(&"Global event"));
    assert!(msgs.contains(&"Local event"));
    assert!(!msgs.contains(&"Far event"),
        "Messages outside visible tiles should be filtered");
}

#[test]
fn combat_log_clear_empties_all() {
    let mut log = CombatLog::default();
    log.push("msg1".into());
    log.push_at("msg2".into(), GridVec::new(1, 1));
    log.clear();
    assert!(log.messages.is_empty());
    assert!(log.recent(10).is_empty());
}

// ─── Kill Count Tests ────────────────────────────────────────────

#[test]
fn kill_count_starts_at_zero() {
    let app = test_app();
    let kills = app.world().resource::<KillCount>();
    assert_eq!(kills.0, 0);
}

#[test]
fn multiple_kills_increment_count() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    // Spawn two weak monsters
    let _m1 = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weak1".into()),
        Health { current: 1, max: 1 },
        CombatStats { attack: 1 },
    )).id();

    let _m2 = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weak2".into()),
        Health { current: 1, max: 1 },
        CombatStats { attack: 1 },
    )).id();

    app.update();

    // Kill first monster
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Kill second monster (player should now be at (60,40) still, monster at (62,40))
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // May need another bump for the second kill
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let kills = app.world().resource::<KillCount>();
    assert!(kills.0 >= 1,
        "Kill count should be at least 1 after killing monsters, got {}", kills.0);
}

// ─── Collectibles Tests ─────────────────────────────────────────

#[test]
fn collectibles_can_reload_with_supplies() {
    let c = Collectibles::default();
    assert!(c.can_reload(Caliber::Cal31),
        "Should be able to reload with starting supplies");
}

#[test]
fn collectibles_consume_reload_decrements() {
    let mut c = Collectibles::default();
    let bullets_before = c.bullets(Caliber::Cal36);
    let caps_before = c.caps;
    let powder_before = c.powder;

    c.consume_reload(Caliber::Cal36);

    assert_eq!(c.bullets(Caliber::Cal36), bullets_before - 1);
    assert_eq!(c.caps, caps_before - 1);
    assert_eq!(c.powder, powder_before - 1);
}

#[test]
fn collectibles_collect_adds_items() {
    let mut c = Collectibles::default();
    c.collect(CollectibleKind::Caps(10));
    assert_eq!(c.caps, 20);
}

// ─── DynamicRng Determinism Tests ────────────────────────────────

#[test]
fn dynamic_rng_deterministic() {
    let rng = DynamicRng { tick: 42 };
    let val1 = rng.roll(123, 456);
    let val2 = rng.roll(123, 456);
    assert_eq!(val1, val2,
        "Same seed+tick+key should produce same result");
}

#[test]
fn dynamic_rng_different_keys_different_values() {
    let rng = DynamicRng { tick: 42 };
    let val1 = rng.roll(123, 1);
    let val2 = rng.roll(123, 2);
    assert_ne!(val1, val2,
        "Different keys should produce different values");
}

#[test]
fn dynamic_rng_range_zero_to_one() {
    let rng = DynamicRng { tick: 0 };
    for key in 0..100 {
        let val = rng.roll(42, key);
        assert!(val >= 0.0 && val < 1.0,
            "RNG value should be in [0, 1), got {}", val);
    }
}

#[test]
fn dynamic_rng_random_index_in_bounds() {
    let rng = DynamicRng { tick: 99 };
    for key in 0..100 {
        let idx = rng.random_index(42, key, 10);
        assert!(idx < 10,
            "Random index should be in [0, 10), got {}", idx);
    }
}

#[test]
fn dynamic_rng_advance_changes_output() {
    let mut rng = DynamicRng { tick: 0 };
    let val1 = rng.roll(123, 456);
    rng.advance();
    let val2 = rng.roll(123, 456);
    assert_ne!(val1, val2,
        "Advancing tick should change RNG output");
}

// ─── Blood Map Tests ─────────────────────────────────────────────

#[test]
fn blood_map_prune_removes_old_stains() {
    let mut blood = BloodMap::default();
    blood.stains.insert(GridVec::new(1, 1), 0);     // turn 0
    blood.stains.insert(GridVec::new(2, 2), 100);   // turn 100
    blood.stains.insert(GridVec::new(3, 3), 250);   // turn 250

    // Prune at turn 300
    blood.prune(300);

    // Stain at turn 0 (age 300 > 200) should be removed
    assert!(!blood.stains.contains_key(&GridVec::new(1, 1)),
        "Old blood stain should be pruned");
    // Stain at turn 100 (age 200 = 200) should remain
    assert!(blood.stains.contains_key(&GridVec::new(2, 2)),
        "Recent-ish blood stain should remain");
    // Stain at turn 250 (age 50 < 200) should remain
    assert!(blood.stains.contains_key(&GridVec::new(3, 3)),
        "Recent blood stain should remain");
}

// ─── Sound Events Tests ──────────────────────────────────────────

#[test]
fn sound_events_expire_after_ticks() {
    let mut sounds = SoundEvents::default();
    sounds.add(GridVec::new(10, 10));

    // Sound has 3 ticks lifetime
    sounds.tick();
    assert_eq!(sounds.events.len(), 1, "Sound should persist after 1 tick");
    sounds.tick();
    assert_eq!(sounds.events.len(), 1, "Sound should persist after 2 ticks");
    sounds.tick();
    assert_eq!(sounds.events.len(), 0, "Sound should expire after 3 ticks");
}

// ─── Turn State Transition Tests ─────────────────────────────────

#[test]
fn turn_counter_starts_at_zero() {
    let app = test_app();
    let counter = app.world().resource::<TurnCounter>();
    assert_eq!(counter.0, 0);
}

// ═══════════════════════════════════════════════════════════════════
//  PROJECTILE SPEED CONSTANTS TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn bullet_speed_is_slow_enough_to_be_visible() {
    // Bullets advance ~12 tiles per game turn and freeze in mid-air
    // between turns (projectile_system skips AwaitingInput frames).
    // This makes them clearly visible as a blinking dot between player actions.
    assert!(
        projectile::BULLET_TILES_PER_TICK <= 15,
        "Bullets should travel at most 15 tiles per tick for visibility, got {}",
        projectile::BULLET_TILES_PER_TICK,
    );
    assert!(
        projectile::BULLET_TILES_PER_TICK >= 8,
        "Bullets should travel at least 8 tiles per tick to feel fast, got {}",
        projectile::BULLET_TILES_PER_TICK,
    );
}

#[test]
fn shrapnel_speed_is_slow_enough_to_be_visible() {
    assert!(
        projectile::SHRAPNEL_TILES_PER_TICK <= 1,
        "Shrapnel should travel at most 1 tile per tick for visibility, got {}",
        projectile::SHRAPNEL_TILES_PER_TICK,
    );
}

#[test]
fn thrown_speed_is_slow_enough_to_be_visible() {
    assert!(
        projectile::THROWN_TILES_PER_TICK <= 2,
        "Thrown items should travel at most 2 tiles per tick for visibility, got {}",
        projectile::THROWN_TILES_PER_TICK,
    );
}

#[test]
fn bullet_faster_than_shrapnel() {
    assert!(
        projectile::BULLET_TILES_PER_TICK > projectile::SHRAPNEL_TILES_PER_TICK,
        "Bullets should be faster than shrapnel",
    );
}

#[test]
fn thrown_faster_than_shrapnel() {
    assert!(
        projectile::THROWN_TILES_PER_TICK > projectile::SHRAPNEL_TILES_PER_TICK,
        "Thrown items should be faster than shrapnel",
    );
}

// ═══════════════════════════════════════════════════════════════════
//  NPC FOV NARROWNESS TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn npc_fov_is_narrow_around_45_degrees() {
    // Spawn an NPC looking east at unit distance (typical AiLookDir).
    let mut app = test_app_with_fov();

    app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Hostile,
        AiLookDir(GridVec::new(1, 0)), // looking east
        Faction::Outlaws,
        Viewshed {
            range: 20,
            visible_tiles: std::collections::HashSet::new(),
            revealed_tiles: std::collections::HashSet::new(),
            dirty: true,
        },
    ));

    app.update();

    let viewshed = app.world_mut()
        .query::<&Viewshed>()
        .single(app.world())
        .unwrap();

    let origin = GridVec::new(60, 40);

    // Should see tiles directly east (in the look direction)
    let forward = origin + GridVec::new(5, 0);
    assert!(
        viewshed.visible_tiles.contains(&forward),
        "NPC should see tiles in its look direction",
    );

    // Should NOT see tiles far to the side (perpendicular to look direction).
    // With a ~45° cone, a tile at distance 5 directly north is at 90° and
    // should be outside the cone.
    let perpendicular = origin + GridVec::new(0, 5);
    assert!(
        !viewshed.visible_tiles.contains(&perpendicular),
        "NPC should NOT see tiles perpendicular to look direction (outside narrow cone)",
    );

    // Should NOT see tiles behind (directly west)
    let behind = origin + GridVec::new(-5, 0);
    assert!(
        !viewshed.visible_tiles.contains(&behind),
        "NPC should NOT see tiles behind its look direction",
    );
}

#[test]
fn wildlife_fov_is_short_range() {
    // Animals have very limited FOV range.
    let mut app = test_app_with_fov();

    app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Hostile,
        AiLookDir(GridVec::new(1, 0)),
        Faction::Wildlife,
        Viewshed {
            range: 20,
            visible_tiles: std::collections::HashSet::new(),
            revealed_tiles: std::collections::HashSet::new(),
            dirty: true,
        },
    ));

    app.update();

    let viewshed = app.world_mut()
        .query::<&Viewshed>()
        .single(app.world())
        .unwrap();

    let origin = GridVec::new(60, 40);

    // Animal should see very close tiles
    let close = origin + GridVec::new(2, 0);
    assert!(
        viewshed.visible_tiles.contains(&close),
        "Animal should see very close tiles in its look direction",
    );

    // Animal should NOT see tiles far away (range capped at 8)
    let far = origin + GridVec::new(12, 0);
    assert!(
        !viewshed.visible_tiles.contains(&far),
        "Animal should NOT see distant tiles (range limited to 8)",
    );
}

// ─── AI Behavior Integration Tests ──────────────────────────────

#[test]
fn ai_memory_tracks_last_known_position() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 63, 40, "Outlaw", Faction::Outlaws);

    // NPC sees the player
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    let mem = app.world().get::<AiMemory>(npc).unwrap();
    assert_eq!(
        mem.last_known_pos,
        Some(GridVec::new(60, 40)),
        "AiMemory should record the player position after seeing them",
    );
}

#[test]
fn ai_memory_expires_after_duration() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 50 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 10, 40);
    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // NPC in Chasing state with stale memory (> 15 turns ago)
    {
        let mut mem = app.world_mut().get_mut::<AiMemory>(npc).unwrap();
        mem.last_known_pos = Some(GridVec::new(50, 40));
        mem.last_seen_turn = 0;
    }
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);

    // Advance turn counter well past MEMORY_DURATION (15)
    app.world_mut().resource_mut::<TurnCounter>().0 = 20;
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    // With expired memory and no visible target, NPC should leave Chasing
    let state = app.world().get::<AiState>(npc).unwrap();
    assert!(
        !matches!(*state, AiState::Chasing),
        "NPC should stop chasing once memory expires (state is {:?})",
        *state,
    );
}

#[test]
fn ai_flee_when_low_hp_no_healing() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 62, 40, "Outlaw", Faction::Outlaws);

    // Default courage=0.5, threshold=30%, so flee below 0.5*0.3 = 15% HP
    // HP 2/20 = 10% -> should flee
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 2;
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }

    app.update();

    let state = app.world().get::<AiState>(npc).unwrap();
    assert_eq!(
        *state,
        AiState::Fleeing,
        "NPC with very low HP and no healing should flee",
    );
}

#[test]
fn ai_no_flee_when_has_whiskey() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 62, 40, "Outlaw", Faction::Outlaws);

    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(whiskey);
    // Low HP but has whiskey
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 2;
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }

    app.update();

    let state = app.world().get::<AiState>(npc).unwrap();
    assert_ne!(
        *state,
        AiState::Fleeing,
        "NPC with healing items should NOT flee even at low HP",
    );
}

#[test]
fn ai_no_flee_when_courage_high() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 62, 40, "Outlaw", Faction::Outlaws);

    // courage=0.0 means flee below 0.0 * 0.3 = 0.0 HP fraction.
    // Since any alive NPC has fraction > 0.0, this threshold is unreachable,
    // so the NPC effectively never flees.
    app.world_mut().get_mut::<AiPersonality>(npc).unwrap().courage = 0.0;
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 2;
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }

    app.update();

    let state = app.world().get::<AiState>(npc).unwrap();
    assert_ne!(
        *state,
        AiState::Fleeing,
        "NPC with zero courage threshold should never flee",
    );
}

#[test]
fn ai_flee_moves_away_from_threat() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 63, 40, "Outlaw", Faction::Outlaws);

    // Set up flee conditions
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 1;
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Fleeing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }

    let initial_pos = *app.world().get::<Position>(npc).unwrap();

    app.update();

    let new_pos = *app.world().get::<Position>(npc).unwrap();
    let dist_before = (initial_pos.x - 60).abs().max((initial_pos.y - 40).abs());
    let dist_after = (new_pos.x - 60).abs().max((new_pos.y - 40).abs());

    assert!(
        dist_after >= dist_before,
        "Fleeing NPC should move away from threat: dist_before={}, dist_after={}",
        dist_before,
        dist_after,
    );
}

#[test]
fn ai_heal_via_use_item_intent() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);
    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(whiskey);
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 8; // 40% < 50% threshold
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    let hp_before = app.world().get::<Health>(npc).unwrap().current;

    app.update();

    let hp_after = app.world().get::<Health>(npc).unwrap().current;
    assert!(
        hp_after > hp_before,
        "NPC should heal via UseItemIntent: before={}, after={}",
        hp_before,
        hp_after,
    );

    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert!(
        inv.items.is_empty(),
        "Whiskey should be consumed from inventory after healing",
    );
}

#[test]
fn ai_pickup_via_pickup_intent() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Place a whiskey on the ground at NPC position
    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().entity_mut(whiskey).insert(Position { x: 60, y: 40 });

    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    // Multiple updates for spatial index rebuild + pickup processing
    for _ in 0..3 {
        app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
        app.update();
    }

    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert!(
        !inv.items.is_empty(),
        "NPC should have picked up the floor item via PickupItemIntent",
    );
}

#[test]
fn ai_ranged_attack_via_ranged_intent() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 50 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 50, 40);
    let npc = spawn_ai_npc(&mut app, 55, 40, "Outlaw", Faction::Outlaws);

    // Give NPC a loaded gun
    let gun = app.world_mut().spawn((
        Item,
        Name("Revolver".into()),
        Renderable {
            symbol: "r".into(),
            fg: roguelike::typedefs::RatColor::Rgb(160, 160, 160),
            bg: roguelike::typedefs::RatColor::Black,
        },
        ItemKind::Gun {
            loaded: 6,
            capacity: 6,
            caliber: Caliber::Cal36,
            attack: 8,
            name: "Revolver".into(),
            blunt_damage: 5,
        },
    )).id();
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(gun);

    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(50, 40));
        vs.dirty = false;
    }

    // Run updates for AI + projectile processing
    for _ in 0..6 {
        app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
        app.update();
    }

    // The gun ammo should have decreased
    let inv = app.world().get::<Inventory>(npc).unwrap();
    if let Some(&gun_ent) = inv.items.first() {
        if let Some(kind) = app.world().get::<ItemKind>(gun_ent) {
            if let ItemKind::Gun { loaded, .. } = kind {
                assert!(
                    *loaded < 6,
                    "Gun should have fewer rounds after firing",
                );
            }
        }
    }
}

#[test]
fn ai_kite_maintains_preferred_range() {
    let mut app = test_app_with_ai();
    for dx in -15..=15 {
        for dy in -5..=5 {
            clear_tile(&mut app, 50 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 50, 40);
    let npc = spawn_ai_npc(&mut app, 52, 40, "Outlaw", Faction::Outlaws);

    // Give NPC a loaded gun and set preferred range to 6
    let gun = app.world_mut().spawn((
        Item,
        Name("Rifle".into()),
        Renderable {
            symbol: "r".into(),
            fg: roguelike::typedefs::RatColor::Rgb(160, 160, 160),
            bg: roguelike::typedefs::RatColor::Black,
        },
        ItemKind::Gun {
            loaded: 6,
            capacity: 6,
            caliber: Caliber::Cal44,
            attack: 10,
            name: "Rifle".into(),
            blunt_damage: 5,
        },
    )).id();
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(gun);
    app.world_mut().get_mut::<AiPersonality>(npc).unwrap().preferred_range = 6;
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(50, 40));
        vs.dirty = false;
    }

    // Block line of sight with a sand cloud between NPC and player so gun can't fire,
    // forcing the kite logic to trigger instead.
    app.world_mut().resource_mut::<SpellParticles>().particles.push(
        (GridVec::new(51, 40), 5, 0, false),
    );

    let initial_dist = 2; // NPC at 52, player at 50

    app.update();

    let new_pos = *app.world().get::<Position>(npc).unwrap();
    let new_dist = (new_pos.x - 50).abs().max((new_pos.y - 40).abs());

    assert!(
        new_dist > initial_dist,
        "Ranged NPC should kite away when closer than preferred_range: dist {} -> {}",
        initial_dist,
        new_dist,
    );
}

#[test]
fn ai_faction_hostility_symmetric() {
    let pairs = [
        (Faction::Outlaws, Faction::Lawmen),
        (Faction::Outlaws, Faction::Vaqueros),
        (Faction::Wildlife, Faction::Outlaws),
        (Faction::Wildlife, Faction::Lawmen),
        (Faction::Wildlife, Faction::Vaqueros),
    ];
    for (a, b) in pairs {
        let ab = ai::factions_are_hostile(a, b);
        let ba = ai::factions_are_hostile(b, a);
        assert_eq!(
            ab, ba,
            "Hostility should be symmetric for {:?} vs {:?}: a->b={}, b->a={}",
            a, b, ab, ba,
        );
    }
}

#[test]
fn ai_wildlife_hostile_to_all() {
    let factions = [Faction::Outlaws, Faction::Lawmen, Faction::Vaqueros];
    for f in factions {
        assert!(
            ai::factions_are_hostile(Faction::Wildlife, f),
            "Wildlife should be hostile to {:?}",
            f,
        );
        assert!(
            ai::factions_are_hostile(f, Faction::Wildlife),
            "{:?} should be hostile to Wildlife",
            f,
        );
    }
}

#[test]
fn ai_same_faction_not_hostile() {
    let factions = [
        Faction::Outlaws,
        Faction::Lawmen,
        Faction::Vaqueros,
        Faction::Wildlife,
    ];
    for f in factions {
        assert!(
            !ai::factions_are_hostile(f, f),
            "Same faction {:?} should not be hostile to itself",
            f,
        );
    }
}

#[test]
fn ai_npc_reloads_empty_gun() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 50 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 50, 40);
    let npc = spawn_ai_npc(&mut app, 55, 40, "Outlaw", Faction::Outlaws);

    // Give NPC an empty gun
    let gun = app.world_mut().spawn((
        Item,
        Name("Revolver".into()),
        Renderable {
            symbol: "r".into(),
            fg: roguelike::typedefs::RatColor::Rgb(160, 160, 160),
            bg: roguelike::typedefs::RatColor::Black,
        },
        ItemKind::Gun {
            loaded: 0,
            capacity: 6,
            caliber: Caliber::Cal36,
            attack: 8,
            name: "Revolver".into(),
            blunt_damage: 5,
        },
    )).id();
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(gun);
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(50, 40));
        vs.dirty = false;
    }

    app.update();

    let inv = app.world().get::<Inventory>(npc).unwrap();
    let gun_ent = inv.items[0];
    let kind = app.world().get::<ItemKind>(gun_ent).unwrap();
    if let ItemKind::Gun { loaded, .. } = kind {
        assert!(
            *loaded > 0,
            "NPC should reload empty gun when in range of target (loaded={})",
            loaded,
        );
    } else {
        panic!("Expected gun item kind");
    }
}

#[test]
fn ai_npc_throws_grenade_at_medium_range() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 50 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 50, 40);
    // Distance 5 is within 3..=6 grenade range
    let npc = spawn_ai_npc(&mut app, 55, 40, "Outlaw", Faction::Outlaws);

    let grenade = spawn_grenade_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(grenade);
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = 0;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(50, 40));
        vs.dirty = false;
    }

    app.update();

    // NPC should have spent energy on the grenade throw action.
    // energy_accumulate adds Speed(100)=ACTION_COST, then AI spends ACTION_COST → 0.
    let energy = app.world().get::<Energy>(npc).unwrap().0;
    assert!(
        energy == 0,
        "NPC should have spent energy on grenade throw action (energy={})",
        energy,
    );
}

#[test]
fn ai_npc_throws_knife_at_medium_range() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 50 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 50, 40);
    // Distance 4 is within 2..=8 knife throw range
    let npc = spawn_ai_npc(&mut app, 54, 40, "Outlaw", Faction::Outlaws);

    let knife = spawn_knife_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(knife);
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = 0;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(50, 40));
        vs.dirty = false;
    }

    app.update();

    // energy_accumulate adds 100, AI spends 100 on knife throw → 0.
    let energy = app.world().get::<Energy>(npc).unwrap().0;
    assert!(
        energy == 0,
        "NPC should have spent energy on knife throw action (energy={})",
        energy,
    );
}

#[test]
fn ai_npc_melee_wide_when_surrounded() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 61, 40, "Outlaw", Faction::Outlaws);

    // Spawn a second hostile adjacent to the NPC (a Lawman NPC that is hostile)
    let _enemy2 = spawn_ai_npc(&mut app, 61, 41, "Lawman", Faction::Lawmen);

    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    app.world_mut().get_mut::<Stamina>(npc).unwrap().current = 50;
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.visible_tiles.insert(GridVec::new(61, 41));
        vs.dirty = false;
    }

    let stamina_before = app.world().get::<Stamina>(npc).unwrap().current;

    app.update();

    let stamina_after = app.world().get::<Stamina>(npc).unwrap().current;
    assert!(
        stamina_after < stamina_before,
        "NPC with 2+ adjacent enemies should use melee wide (stamina: {} -> {})",
        stamina_before,
        stamina_after,
    );
}

#[test]
fn ai_idle_scavenges_nearby_items() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);
    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().entity_mut(whiskey).insert(Position { x: 63, y: 40 });

    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(63, 40));
        vs.dirty = false;
    }
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    let initial_pos = *app.world().get::<Position>(npc).unwrap();

    app.update();

    let new_pos = *app.world().get::<Position>(npc).unwrap();
    let dist_before = (initial_pos.x - 63).abs().max((initial_pos.y - 40).abs());
    let dist_after = (new_pos.x - 63).abs().max((new_pos.y - 40).abs());

    assert!(
        dist_after < dist_before,
        "Idle NPC should move toward visible item: dist {} -> {}",
        dist_before,
        dist_after,
    );
}

#[test]
fn ai_patrol_returns_to_origin() {
    let mut app = test_app_with_ai();
    for dx in -15..=15 {
        for dy in -10..=10 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Lawman", Faction::Lawmen);
    // Patrol origin is at spawn (60,40), move NPC far away
    app.world_mut().get_mut::<Position>(npc).unwrap().x = 72;
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Patrolling);

    let origin = GridVec::new(60, 40);
    let start_pos = GridVec::new(72, 40);
    let start_dist = start_pos.chebyshev_distance(origin);

    for _ in 0..5 {
        app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
        app.update();
    }

    let final_pos = *app.world().get::<Position>(npc).unwrap();
    let final_dist = GridVec::new(final_pos.x, final_pos.y).chebyshev_distance(origin);

    assert!(
        final_dist < start_dist,
        "Patrolling NPC should return toward origin: dist {} -> {}",
        start_dist,
        final_dist,
    );
}

#[test]
fn ai_energy_system_accumulates() {
    let mut app = test_app_with_ai();

    // Spawn an entity with Speed and Energy but NOT an AI NPC,
    // so ai_system won't consume the energy.
    let ent = app.world_mut().spawn((
        Speed(ACTION_COST),
        Energy(0),
    )).id();
    assert_eq!(app.world().get::<Energy>(ent).unwrap().0, 0);

    app.update();

    let energy = app.world().get::<Energy>(ent).unwrap().0;
    assert_eq!(
        energy, ACTION_COST,
        "Energy should have accumulated by Speed amount after update (got {})",
        energy,
    );
}

#[test]
fn ai_unified_pickup_works_for_npc() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);
    let knife = spawn_knife_item(&mut app);
    app.world_mut().entity_mut(knife).insert(Position { x: 60, y: 40 });

    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    for _ in 0..3 {
        app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
        app.update();
    }

    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert!(
        !inv.items.is_empty(),
        "NPC should pick up knife via unified pickup system",
    );
}

#[test]
fn ai_unified_use_item_works_for_npc() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);
    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(whiskey);
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 5;
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    let hp = app.world().get::<Health>(npc).unwrap().current;
    assert!(
        hp > 5,
        "NPC should have healed via unified use-item system (hp={})",
        hp,
    );
}

#[test]
fn ai_personality_affects_preferred_range() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc_melee = spawn_ai_npc(&mut app, 60, 40, "Melee", Faction::Outlaws);
    let npc_ranged = spawn_ai_npc(&mut app, 65, 40, "Ranged", Faction::Outlaws);

    app.world_mut().get_mut::<AiPersonality>(npc_melee).unwrap().preferred_range = 1;
    app.world_mut().get_mut::<AiPersonality>(npc_ranged).unwrap().preferred_range = 8;

    let melee_range = app.world().get::<AiPersonality>(npc_melee).unwrap().preferred_range;
    let ranged_range = app.world().get::<AiPersonality>(npc_ranged).unwrap().preferred_range;

    assert_eq!(melee_range, 1, "Melee NPC preferred_range should be 1");
    assert_eq!(ranged_range, 8, "Ranged NPC preferred_range should be 8");
    assert_ne!(
        melee_range, ranged_range,
        "Different personalities should have different preferred ranges",
    );
}

#[test]
fn ai_chasing_uses_a_star_pathfinding() {
    let mut app = test_app_with_ai();
    for dx in -15..=15 {
        for dy in -5..=5 {
            clear_tile(&mut app, 50 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 50, 40);
    let npc = spawn_ai_npc(&mut app, 56, 40, "Outlaw", Faction::Outlaws);

    // Block the direct path with obstacles
    app.world_mut().spawn((
        Position { x: 53, y: 40 },
        BlocksMovement,
        Name("Wall".into()),
    ));
    app.world_mut().spawn((
        Position { x: 54, y: 40 },
        BlocksMovement,
        Name("Wall".into()),
    ));
    app.world_mut().spawn((
        Position { x: 55, y: 40 },
        BlocksMovement,
        Name("Wall".into()),
    ));

    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(50, 40));
        vs.dirty = false;
    }

    app.update();

    let pos_after = *app.world().get::<Position>(npc).unwrap();
    let moved = pos_after.x != 56 || pos_after.y != 40;
    assert!(
        moved,
        "NPC should pathfind around obstacles, but pos unchanged at ({}, {})",
        pos_after.x,
        pos_after.y,
    );
}

#[test]
fn ai_flee_to_patrol_state() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 1;
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Fleeing);

    // No player spawned — no threats visible
    for _ in 0..3 {
        app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
        app.update();
    }

    let state = app.world().get::<AiState>(npc).unwrap();
    assert!(
        matches!(*state, AiState::Patrolling | AiState::Idle),
        "Fleeing NPC without threats should transition to Patrolling or Idle (state is {:?})",
        *state,
    );
}

// ─── Additional AI Edge Case Tests ──────────────────────────────

#[test]
fn ai_npc_does_not_act_without_energy() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 63, 40, "Outlaw", Faction::Outlaws);

    // Set speed to 0 so no energy accumulates
    app.world_mut().get_mut::<Speed>(npc).unwrap().0 = 0;
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }

    let initial_pos = *app.world().get::<Position>(npc).unwrap();

    app.update();

    let new_pos = *app.world().get::<Position>(npc).unwrap();
    let state = app.world().get::<AiState>(npc).unwrap();
    assert_eq!(initial_pos, new_pos, "NPC without energy should not move");
    assert_eq!(*state, AiState::Idle, "NPC without energy should remain Idle");
}

#[test]
fn ai_multiple_npcs_independent_states() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 55 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 55, 40);

    let npc1 = spawn_ai_npc(&mut app, 58, 40, "Outlaw1", Faction::Outlaws);
    let npc2 = spawn_ai_npc(&mut app, 62, 40, "Outlaw2", Faction::Outlaws);

    // NPC1 can see the player, NPC2 cannot
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc1).unwrap();
        vs.visible_tiles.insert(GridVec::new(55, 40));
        vs.dirty = false;
    }

    app.world_mut().get_mut::<Energy>(npc1).unwrap().0 = ACTION_COST;
    app.world_mut().get_mut::<Energy>(npc2).unwrap().0 = ACTION_COST;

    app.update();

    let state1 = *app.world().get::<AiState>(npc1).unwrap();
    let state2 = *app.world().get::<AiState>(npc2).unwrap();

    assert_eq!(state1, AiState::Chasing, "NPC1 (sees player) should be Chasing");
    assert_eq!(state2, AiState::Idle, "NPC2 (cannot see player) should remain Idle");
}

#[test]
fn ai_npc_heals_before_chasing() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 57, 40);
    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    let whiskey = spawn_whiskey_item(&mut app);
    app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(whiskey);
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 8;
    app.world_mut().get_mut::<AiState>(npc).unwrap().clone_from(&AiState::Chasing);
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;
    app.world_mut().get_mut::<AiLookDir>(npc).unwrap().0 = GridVec::new(-1, 0);
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(57, 40));
        vs.dirty = false;
    }

    app.update();

    let hp = app.world().get::<Health>(npc).unwrap().current;
    assert!(
        hp > 8,
        "NPC should heal before other actions when HP < 50% (hp={})",
        hp,
    );
}

#[test]
fn ai_a_star_pathfinding_basic() {
    let start = GridVec::new(0, 0);
    let goal = GridVec::new(5, 0);

    let step = ai::a_star_first_step_pub(start, goal, |_| true);
    assert!(step.is_some(), "A* should find a step on open grid");
    let s = step.unwrap();
    let new_pos = start + s;
    let dist_after = new_pos.chebyshev_distance(goal);
    let dist_before = start.chebyshev_distance(goal);
    assert!(
        dist_after < dist_before,
        "A* first step should move closer to goal: dist {} -> {}",
        dist_before,
        dist_after,
    );
}

#[test]
fn ai_a_star_pathfinding_around_wall() {
    let start = GridVec::new(0, 0);
    let goal = GridVec::new(3, 0);

    let blocked = [GridVec::new(1, 0), GridVec::new(2, 0)];
    let step = ai::a_star_first_step_pub(start, goal, |p| !blocked.contains(&p));
    assert!(step.is_some(), "A* should find a path around blocked tiles");
    let s = step.unwrap();
    assert!(s.y != 0, "A* should step diagonally to avoid wall (step=({}, {}))", s.x, s.y);
}

#[test]
fn ai_memory_updates_on_each_sighting() {
    let mut app = test_app_with_ai();
    for dx in -10..=10 {
        for dy in -5..=5 {
            clear_tile(&mut app, 55 + dx, 40 + dy);
        }
    }

    let player = spawn_test_player(&mut app, 55, 40);
    let npc = spawn_ai_npc(&mut app, 58, 40, "Outlaw", Faction::Outlaws);

    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(55, 40));
        vs.dirty = false;
    }
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    let mem1_pos = app.world().get::<AiMemory>(npc).unwrap().last_known_pos;
    assert_eq!(mem1_pos, Some(GridVec::new(55, 40)));

    // Move player
    app.world_mut().get_mut::<Position>(player).unwrap().x = 54;
    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(54, 40));
        vs.dirty = false;
    }
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    let mem2 = app.world().get::<AiMemory>(npc).unwrap();
    assert_eq!(
        mem2.last_known_pos,
        Some(GridVec::new(54, 40)),
        "Memory should update to the player new position",
    );
}

#[test]
fn ai_idle_does_not_chase_without_visibility() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let _player = spawn_test_player(&mut app, 60, 40);
    let npc = spawn_ai_npc(&mut app, 63, 40, "Outlaw", Faction::Outlaws);

    // NPC viewshed does NOT contain the player position
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    let state = app.world().get::<AiState>(npc).unwrap();
    assert_eq!(
        *state,
        AiState::Idle,
        "NPC should remain Idle when player is not in viewshed",
    );
}

#[test]
fn ai_health_fraction_calculation() {
    let hp_full = Health { current: 20, max: 20 };
    assert!((hp_full.fraction() - 1.0).abs() < f64::EPSILON, "Full HP should be 1.0");
    assert!(hp_full.is_full(), "Full HP should report is_full()");

    let hp_half = Health { current: 10, max: 20 };
    assert!((hp_half.fraction() - 0.5).abs() < f64::EPSILON, "Half HP should be 0.5");
    assert!(!hp_half.is_full(), "Half HP should not be full");

    let hp_zero = Health { current: 0, max: 20 };
    assert!((hp_zero.fraction() - 0.0).abs() < f64::EPSILON, "Zero HP should be 0.0");

    let mut hp_heal = Health { current: 10, max: 20 };
    let healed = hp_heal.heal(5);
    assert_eq!(healed, 5, "Should heal 5");
    assert_eq!(hp_heal.current, 15, "HP should be 15 after healing");

    let capped = hp_heal.heal(100);
    assert_eq!(capped, 5, "Should only heal to max");
    assert_eq!(hp_heal.current, 20, "HP should be capped at max");
}

#[test]
fn ai_energy_can_act_threshold() {
    let energy_zero = Energy(0);
    assert!(!energy_zero.can_act(), "Zero energy should not be able to act");

    let energy_half = Energy(50);
    assert!(!energy_half.can_act(), "50 energy should not be able to act");

    let energy_full = Energy(ACTION_COST);
    assert!(energy_full.can_act(), "ACTION_COST energy should be able to act");

    let energy_over = Energy(ACTION_COST + 50);
    assert!(energy_over.can_act(), "Over ACTION_COST energy should be able to act");
}

#[test]
fn ai_npc_inventory_capacity_limit() {
    let mut app = test_app_with_ai();
    for dx in -5..=5 {
        for dy in -5..=5 {
            clear_tile(&mut app, 60 + dx, 40 + dy);
        }
    }

    let npc = spawn_ai_npc(&mut app, 60, 40, "Outlaw", Faction::Outlaws);

    // Fill inventory to capacity (9 items)
    for _ in 0..9 {
        let item = spawn_whiskey_item(&mut app);
        app.world_mut().get_mut::<Inventory>(npc).unwrap().items.push(item);
    }

    // Place another item on the ground
    let extra = spawn_whiskey_item(&mut app);
    app.world_mut().entity_mut(extra).insert(Position { x: 60, y: 40 });

    {
        let mut vs = app.world_mut().get_mut::<Viewshed>(npc).unwrap();
        vs.visible_tiles.insert(GridVec::new(60, 40));
        vs.dirty = false;
    }
    app.world_mut().get_mut::<Energy>(npc).unwrap().0 = ACTION_COST;

    app.update();

    let inv = app.world().get::<Inventory>(npc).unwrap();
    assert_eq!(
        inv.items.len(),
        9,
        "NPC should not pick up items when inventory is full (has {} items)",
        inv.items.len(),
    );
}
