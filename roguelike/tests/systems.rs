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
use roguelike::systems::{combat, movement, projectile, spatial_index, spell};

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
    app.init_resource::<PendingExp>();
    app.init_resource::<PendingNpcExp>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<CursorPosition>();
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
            combat::level_up_system,
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
        CombatStats { attack: 5, defense: 2 },
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
        CombatStats { attack: 3, defense: 1 },
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

    // Player attack=5, Monster defense=1 → damage=4
    let monster_health = app.world().get::<Health>(monster).unwrap();
    assert_eq!(monster_health.current, 6, "Monster should have taken 4 damage (5 atk - 1 def)");
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

    // Monster attack=3, Player defense=2 → damage=1
    let player_health = app.world().get::<Health>(player).unwrap();
    assert_eq!(player_health.current, 29, "Player should have taken 1 damage (3 atk - 2 def)");
}

#[test]
fn no_damage_when_defense_exceeds_attack() {
    let mut app = test_app();
    // Spawn player with very high defense
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5, defense: 10 },
    )).id();

    // Spawn weak monster
    let monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Rat".into()),
        Health { current: 5, max: 5 },
        CombatStats { attack: 2, defense: 0 },
    )).id();

    app.update();

    // Monster attacks player: attack=2, defense=10 → damage=0
    app.world_mut().write_message(MoveIntent {
        entity: monster,
        dx: -1,
        dy: 0,
    });
    app.update();

    let player_health = app.world().get::<Health>(player).unwrap();
    assert_eq!(player_health.current, 30, "Player should take no damage from weak monster");
}

// ─── Death system tests ──────────────────────────────────────────

#[test]
fn monster_dies_at_zero_health() {
    let mut app = test_app();
    // Spawn a monster with 1 HP and 0 defense
    let player = spawn_test_player(&mut app, 60, 40);
    let monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weakling".into()),
        Health { current: 1, max: 1 },
        CombatStats { attack: 1, defense: 0 },
    )).id();

    app.update();

    // Player attacks monster: attack=5, defense=0 → damage=5, kills the 1HP monster
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
        CombatStats { attack: 1, defense: 0 },
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
        CombatStats { attack: 1, defense: 0 },
    )).id();

    // Monster with defense >= player attack
    let _monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("IronGolem".into()),
        Health { current: 50, max: 50 },
        CombatStats { attack: 1, defense: 10 },
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
        "Combat log should record 'no damage' message when attack <= defense"
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

    // First attack: 5 - 1 = 4 damage → 10 - 4 = 6 HP
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let hp1 = app.world().get::<Health>(monster).unwrap().current;
    assert_eq!(hp1, 6);

    // Second attack
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    let hp2 = app.world().get::<Health>(monster).unwrap().current;
    assert_eq!(hp2, 2, "Second attack should further reduce HP");
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
    app.init_resource::<PendingExp>();
    app.init_resource::<PendingNpcExp>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<SpellParticles>();
    app.init_resource::<CursorPosition>();
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
            combat::apply_damage_system,
            combat::death_system,
            combat::level_up_system,
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
        CombatStats { attack: 5, defense: 2 },
    )).id();

    // Monster within spell radius (2 tiles away, radius=3)
    let monster = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Goblin".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3, defense: 1 },
    )).id();

    app.update();

    // Cast spell with radius 3
    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns shrapnel projectile entities
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
        CombatStats { attack: 5, defense: 2 },
    )).id();

    // Monster far outside spell radius
    let monster = app.world_mut().spawn((
        Position { x: 70, y: 40 },
        Hostile,
        BlocksMovement,
        Name("FarGoblin".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3, defense: 1 },
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
        CombatStats { attack: 5, defense: 2 },
    )).id();

    // Two monsters within radius
    let m1 = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Goblin1".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3, defense: 1 },
    )).id();

    let m2 = app.world_mut().spawn((
        Position { x: 60, y: 41 },
        Hostile,
        BlocksMovement,
        Name("Goblin2".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 3, defense: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns shrapnel
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
        CombatStats { attack: 5, defense: 2 },
    )).id();

    // Weak monster that will die from shrapnel damage
    let monster = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Weakling".into()),
        Health { current: 3, max: 3 },
        CombatStats { attack: 1, defense: 0 },
    )).id();

    app.update();

    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns shrapnel
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
        CombatStats { attack: 1, defense: 0 },
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
        CombatStats { attack: 5, defense: 2 },
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

    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("grenade")),
        "Combat log should note grenade was thrown"
    );
}

// ─── Hell Gate tests ─────────────────────────────────────────────

#[test]
fn player_can_bump_attack_hell_gate() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    // Spawn a hell gate adjacent to the player
    let gate = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        HellGate,
        Hostile,
        BlocksMovement,
        Name("Gate of Hell".into()),
        Health { current: 100, max: 100 },
        CombatStats { attack: 0, defense: 3 },
    )).id();

    app.update();

    // Player bumps into gate → should trigger attack
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Player attack=5, Gate defense=3 → damage=2
    let gate_health = app.world().get::<Health>(gate).unwrap();
    assert_eq!(gate_health.current, 98, "Gate should have taken 2 damage (5 atk - 3 def)");
}

#[test]
fn gate_destruction_triggers_victory() {
    let mut app = test_app();
    let player = spawn_test_player(&mut app, 60, 40);

    // Spawn a gate with just 1 HP so it dies in one hit
    let gate = app.world_mut().spawn((
        Position { x: 61, y: 40 },
        HellGate,
        Hostile,
        BlocksMovement,
        Name("Gate of Hell".into()),
        Health { current: 1, max: 100 },
        CombatStats { attack: 0, defense: 0 },
    )).id();

    app.update();

    // Player attacks the gate
    app.world_mut().write_message(MoveIntent {
        entity: player,
        dx: 1,
        dy: 0,
    });
    app.update();

    // Gate should be despawned
    assert!(
        app.world().get::<Health>(gate).is_none(),
        "Gate should be despawned after reaching 0 HP"
    );

    // Victory message should be in the combat log
    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("Enemy Stronghold crumbles")),
        "Combat log should contain victory message"
    );
}

#[test]
fn spell_damages_hell_gate() {
    let mut app = test_app_with_spells();
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5, defense: 2 },
    )).id();

    // Gate within spell radius
    let gate = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        HellGate,
        Hostile,
        BlocksMovement,
        Name("Gate of Hell".into()),
        Health { current: 100, max: 100 },
        CombatStats { attack: 0, defense: 3 },
    )).id();

    app.update();

    // Cast spell with radius 3
    app.world_mut().write_message(SpellCastIntent {
        caster: player,
        radius: 3,
        target: GridVec::new(60, 40),
        grenade_index: 0,
    });
    app.update(); // spell_system spawns shrapnel
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
        CombatStats { attack: 1, defense: 0 },
    )).id();

    let e2 = app.world_mut().spawn((
        Position { x: 61, y: 42 },
        BlocksMovement,
        Name("E2".into()),
        Health { current: 10, max: 10 },
        CombatStats { attack: 1, defense: 0 },
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
    app.init_resource::<PendingExp>();
    app.init_resource::<PendingNpcExp>();
    app.init_resource::<SoundEvents>();
    app.init_resource::<SpellParticles>();
    app.init_resource::<CursorPosition>();
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
            combat::level_up_system,
        )
            .chain(),
    );
    app
}

/// Spawns a player with ammo at the given position.
fn spawn_test_player_with_ammo(app: &mut App, x: i32, y: i32, ammo: i32) -> Entity {
    app.world_mut().spawn((
        Position { x, y },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 5, defense: 2 },
        Ammo { current: ammo, max: 30 },
    )).id()
}

#[test]
fn ranged_attack_consumes_ammo() {
    let mut app = test_app_with_ranged();
    let player = spawn_test_player_with_ammo(&mut app, 60, 40, 10);
    let _monster = spawn_test_monster(&mut app, 64, 40, "Bandit");

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: None,
    });
    app.update();

    let ammo = app.world().get::<Ammo>(player).unwrap();
    assert_eq!(ammo.current, 9, "Ranged attack should consume 1 ammo");
}

#[test]
fn ranged_attack_no_ammo_does_not_fire() {
    let mut app = test_app_with_ranged();
    let player = spawn_test_player_with_ammo(&mut app, 60, 40, 0);
    let monster = spawn_test_monster(&mut app, 64, 40, "Bandit");

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: None,
    });
    app.update();

    // Monster should not be damaged.
    let monster_hp = app.world().get::<Health>(monster).unwrap();
    assert_eq!(monster_hp.current, 10, "Monster should not be damaged when player has no ammo");

    // Combat log should contain ammo message.
    let log = app.world().resource::<CombatLog>();
    assert!(
        log.messages.iter().any(|m| m.contains("ammo")),
        "Combat log should mention ammo shortage"
    );
}

#[test]
fn ranged_attack_damages_nearest_enemy() {
    let mut app = test_app_with_ranged();
    let player = spawn_test_player_with_ammo(&mut app, 60, 40, 10);
    // Monster at distance 4 (within range 8).
    let monster = app.world_mut().spawn((
        Position { x: 64, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Bandit".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3, defense: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: None,
    });
    app.update(); // ranged_attack_system spawns bullet entity
    app.update(); // projectile_system advances bullet and applies damage

    let monster_hp = app.world().get::<Health>(monster).unwrap();
    assert!(monster_hp.current < 20, "Ranged attack should damage the target");
}

#[test]
fn ranged_attack_no_target_in_range() {
    let mut app = test_app_with_ranged();
    let player = spawn_test_player_with_ammo(&mut app, 60, 40, 10);
    // Monster far away (distance 20, beyond range 8).
    let _monster = app.world_mut().spawn((
        Position { x: 80, y: 40 },
        Hostile,
        BlocksMovement,
        Name("FarBandit".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3, defense: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: None,
    });
    app.update(); // ranged_attack_system spawns bullet entity
    app.update(); // projectile_system advances bullet (misses - out of range)

    // Ammo should still be consumed (shot fired but missed).
    let ammo = app.world().get::<Ammo>(player).unwrap();
    assert_eq!(ammo.current, 9, "Ammo should be consumed even if no target in range");

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
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 10, defense: 2 },
        Ammo { current: 10, max: 30 },
    )).id();

    // Two enemies in a line east of player with low defense.
    let m1 = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Bandit1".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3, defense: 2 },
    )).id();

    let m2 = app.world_mut().spawn((
        Position { x: 64, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Bandit2".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3, defense: 2 },
    )).id();

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: None,
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
fn ranged_bullet_stops_when_penetration_exhausted() {
    let mut app = test_app_with_ranged();
    // Player with attack=3, low penetration.
    let player = app.world_mut().spawn((
        Position { x: 60, y: 40 },
        Player,
        BlocksMovement,
        Name("Player".into()),
        Health { current: 30, max: 30 },
        CombatStats { attack: 3, defense: 2 },
        Ammo { current: 10, max: 30 },
    )).id();

    // First enemy with defense=5 (exceeds penetration after first hit).
    let m1 = app.world_mut().spawn((
        Position { x: 62, y: 40 },
        Hostile,
        BlocksMovement,
        Name("TankSoldier".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3, defense: 5 },
    )).id();

    // Second enemy behind the tank.
    let m2 = app.world_mut().spawn((
        Position { x: 64, y: 40 },
        Hostile,
        BlocksMovement,
        Name("Bandit".into()),
        Health { current: 20, max: 20 },
        CombatStats { attack: 3, defense: 1 },
    )).id();

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: None,
    });
    app.update(); // ranged_attack_system spawns bullet entity
    app.update(); // projectile_system advances bullet

    // First enemy should be hit.
    let m1_hp = app.world().get::<Health>(m1).unwrap();
    assert!(m1_hp.current < 20, "First enemy should be hit");

    // Second enemy should NOT be hit — bullet penetration exhausted after high defense target.
    let m2_hp = app.world().get::<Health>(m2).unwrap();
    assert_eq!(m2_hp.current, 20, "Second enemy should not be hit after penetration exhausted by high-defense target");
}

#[test]
fn ranged_attack_logs_shoot_message() {
    let mut app = test_app_with_ranged();
    let player = spawn_test_player_with_ammo(&mut app, 60, 40, 10);
    let _monster = spawn_test_monster(&mut app, 64, 40, "Bandit");

    app.update();

    app.world_mut().write_message(RangedAttackIntent {
        attacker: player,
        range: 8,
        dx: 1,
        dy: 0,
        gun_item: None,
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
    let player = spawn_test_player_with_ammo(&mut app, 60, 40, 10);

    // Adjacent enemies (distance 1).
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
    let player = spawn_test_player_with_ammo(&mut app, 60, 40, 10);

    // Enemy at distance 3 (not adjacent).
    let monster = spawn_test_monster(&mut app, 63, 40, "FarBandit");

    app.update();

    app.world_mut().write_message(MeleeWideIntent {
        attacker: player,
    });
    app.update();

    let monster_hp = app.world().get::<Health>(monster).unwrap();
    assert_eq!(monster_hp.current, 10, "Distant enemy should not be hit by roundhouse kick");
}
