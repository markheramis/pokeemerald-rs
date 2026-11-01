//! Battle subsystem: typed core data, the Gen-3 damage formula, and the
//! turn engine built on top of it.
//!
//! The crate serves S-6. Its base is typed data and a deterministic damage
//! pipeline; the turn engine on top of it is [`battle::Battle`], an owned
//! state machine that plays a full headless single wild battle (action
//! selection → turn resolution → faint/exp → victory/defeat/run) using
//! [`hit::resolve_hit`] to assemble [`accuracy`], [`critical`], and
//! [`damage`] into upstream's exact battle-script step order, plus
//! [`turn_order`] (priority/speed/speed-tie), [`escape`] (the run-away
//! formula), [`exp`] (faint exp gain), [`pokemon::BattlePokemon`] (computed
//! battle stats), and [`wild`] (wild-encounter construction from species +
//! level via the upstream personality/nature/IV RNG draws, plus
//! [`wild::initial_moveset`] — `GiveBoxMonInitialMoveset`, so an overworld
//! encounter derives the *real* wild moveset for a rolled species/level
//! instead of being handed one).
//!
//! Species base stats, move data, and the type-effectiveness chart are
//! already extracted upstream data living in [`assets`] `(no-verbatim)`;
//! [`dex::Dex`] bundles typed access to them for battle-formula callers.
//! This crate adds the upstream tables `assets` does not carry —
//! nature stat modifiers ([`nature`], upstream `gNatureStatTable`) and
//! stat-stage multipliers ([`stat_stage`], upstream `gStatStageRatios`) —
//! plus [`damage`]'s re-implementation of `CalculateBaseDamage`
//! (`pokeemerald/src/pokemon.c`) and the STAB/type-effectiveness/random-roll
//! battle-script steps around it.
//!
//! Move-effect breadth is the sharp edge of this crate, so it is enforced
//! rather than assumed: a move is only executable if its `EFFECT_*` runs
//! one of the battle scripts this crate reproduces — the nine pipelines
//! `battle::ensure_executable` composes:
//!
//! | pipeline | script |
//! |---|---|
//! | [`hit`] | `BattleScript_EffectHit` ([`hit::is_ordinary_hit_effect`]) |
//! | [`stat_change`] | the `BattleScript_EffectStatUp`/`StatDown` family ([`stat_change::is_stat_change_effect`]) |
//! | [`drain`] | `BattleScript_EffectAbsorb` ([`drain::is_drain_effect`]) |
//! | [`fixed_damage`] | `_Sonicboom` / `_DragonRage` / `_LevelDamage` ([`fixed_damage::is_fixed_damage_effect`]) |
//! | [`multi_hit`] | `BattleScript_EffectMultiHit` ([`multi_hit::is_multi_hit_effect`]) |
//! | [`flag_move`] | `_Splash` / `_FocusEnergy` / `_Charge` ([`flag_move::is_flag_move_effect`]) |
//! | [`defense_curl`] | `_EffectDefenseCurl` ([`defense_curl::is_defense_curl_effect`]) |
//! | [`paralyze`] | `BattleScript_EffectParalyze` ([`paralyze::is_paralyze_effect`]) |
//! | [`confuse`] | `BattleScript_EffectConfuse` ([`confuse::is_confuse_effect`]) |
//!
//! The screen is guarded at a two-sided boundary. [`battle::Battle::new`]
//! rejects a battle whose **opposing** mon knows anything else (its
//! rejection loop can land on any slot), while the **player's** moveset may
//! carry unsupported moves and each *chosen* slot is validated per turn
//! instead. Both checks run before any RNG is drawn and before any HP, PP or
//! stage changes, so an unsupported configuration or pick can never leave a
//! half-played turn behind — the reason it matters is that a script this
//! crate does not reproduce computes different damage *and* spends a
//! different number of `Random()` calls, and a shared stream that advanced
//! the wrong number of steps is wrong for the rest of the battle.
//!
//! [`secondary::ensure_admissible`]'s Serene Grace screen is status-dependent
//! rather than static, so a residual that cures a primary status can flip a
//! verdict construction already gave. `Battle` re-screens the enemy's
//! admissible moves once at the start of every turn, before that turn's own
//! RNG, restoring the guarantee above for the life of the battle. The one
//! exception is a guaranteed escape (Run Away, or raw Speed at or above the
//! enemy's): that turn ends before the enemy acts, so it skips the re-screen
//! (`pokeemerald/src/battle_util.c:427`-`:447`).
//!
//! `BATTLE_TYPE_FIRST_BATTLE` — the Route 101 intro
//! Zigzagoon fight's rules — is [`battle::Battle::new`]'s `first_battle`
//! flag: crit suppression ([`hit::resolve_hit`]'s `suppress_crit`, see
//! [`critical`]'s module docs), running forbidden
//! ([`BattleError::RunForbidden`], see `escape`'s module docs), and the
//! wild opponent's narrow AI-branch move choice (`battle`'s private
//! `opponent_ai` submodule, not upstream's general trainer AI, which stays
//! `I-5`). The scripted intro's own construction —
//! `SetUpBattleVarsAndBirchZigzagoon`'s Zigzagoon, built by
//! [`wild::build_pokemon_with_random_personality`] rather than
//! [`wild::build_wild_pokemon`] (see that function's docs for the exact
//! upstream draw-order difference) — and its own headless driver both live
//! in `crates/pokeemerald-rs/src/flow/first_battle.rs`. The "don't leave Prof.
//! Birch!" narrative trigger that upstream reaches it through has no script
//! engine to run it and stays unmodelled (that module's own docs, and the
//! `src/battle_setup.c` ledger entry); every ordinary Route 101 grass
//! encounter still constructs with `first_battle = false`
//! (`crates/pokeemerald-rs/src/flow/wild_encounter.rs`).
//!
//! `BATTLE_TYPE_TRAINER` — the scripted Route 103 rival battle's rules — is
//! a second constructor, [`battle::Battle::new_trainer`],
//! rather than a second flag: a trainer battle needs *state* a wild one does
//! not (a party bench, a prize purse, `gTrainers[].aiFlags`), so that state
//! lives in [`battle::trainer::TrainerContext`] and `Battle` gates every
//! battle-type delta on which shape it was built for. The five deltas —
//! running refused outright ([`BattleError::NoRunningFromTrainer`], a
//! *different* upstream gate from `first_battle`'s), a party opponent, a
//! forced post-faint send-out chosen by `GetMostSuitableMonToSwitchInto`'s
//! suitability passes, `x1.5` experience
//! ([`exp::trainer_faint_exp`]), and prize money on a win
//! ([`battle::BattleEvent::MoneyGained`]) — each carry their upstream
//! citation beside the code that owns them. The opponent's
//! move choice is upstream's real `AI_SCRIPT_*` scoring pipeline
//! (`battle`'s private `trainer_ai` submodule): `AI_CheckBadMove`,
//! `AI_TryToFaint`, `AI_CheckViability` and `AI_SetupFirstTurn`, narrowed to
//! the move effects a level-5 starter can carry and screened at construction
//! so nothing outside that narrowing can silently mis-draw. The battle's own
//! construction — `CreateNPCTrainerParty`'s seeded personalities and fixed
//! IVs — and its headless driver are
//! `crates/pokeemerald-rs/src/flow/npc_trainer_battle.rs`, the same split
//! the first battle uses.
//!
//! Route 103's *sight* trainers use that same construction, screened by
//! [`battle::trainer::ensure_trainer_party_startable`], the trainer-side
//! counterpart of [`wild::ensure_wild_startable`]. Both compose the screens
//! a battle would apply anyway into a pre-flight an integration layer can
//! run **before the first draw** — the only stream-faithful way to refuse a
//! party this engine cannot fight, since a per-frame trigger that refuses
//! *after* `CreateNPCTrainerParty`'s per-mon OT-id draws would spend the
//! shared stream on every frame the player stands in a cone.
//!
//! Four of the pipelines are their own modules because each reproduces a
//! *different* battle script with a different `Random()` spend (the tables
//! in each module's docs are the contract): [`drain`]
//! (`BattleScript_EffectAbsorb`, 3 draws for a landed move where an
//! ordinary one costs 4, because the script never reaches
//! `seteffectwithchance`), [`fixed_damage`] (Sonic Boom / Dragon Rage /
//! Seismic Toss / Night Shade — 2 draws, no crit roll and no damage roll,
//! and a flat figure no stat can move), [`multi_hit`]
//! (`Cmd_setmultihitcounter`'s **two-stage** 2..5 draw, then a crit and a
//! damage roll per hit, then one trailing effect-chance draw for the whole
//! move), and [`flag_move`] (Splash, Focus Energy, Charge — **zero** draws
//! on every path, which [`flag_move::resolve_flag_move`]'s signature
//! enforces by taking no RNG at all).
//!
//! Two supporting concepts serve them. [`secondary`] is the shared
//! post-damage hook — `Cmd_seteffectwithchance` — its own module because
//! the five damaging scripts disagree
//! about whether and when they reach it; it spends the draw exactly where
//! upstream does and **fails closed**
//! ([`BattleError::UnportedSecondaryEffect`]) if a roll ever lands on a
//! `MOVE_EFFECT_*` byte whose infliction is not ported. [`volatile`] carries the two `status2`/`gStatuses3` bits
//! the flag-only moves set, read by [`critical::crit_stage`] (Focus
//! Energy's `+2` crit stages) and [`hit::damage_core`] (Charge's Electric
//! doubling), and ticked down at end of turn.
//!
//! Abilities are modelled with the move family that exposes them rather
//! than as a speculative ability layer: [`ability::pinch_boosts_power`] — Overgrow's `x1.5` power boost
//! at or below a third of maximum HP, applied inside the damage formula —
//! and [`ability::inverts_drain`] — Liquid Ooze turning a drain heal into
//! damage on the attacker, in the drain script's own message and faint
//! order. Two damage-path pairs follow the same rule:
//! [`ability::suppresses_critical_hits`] — Battle Armor and Shell
//! Armor, which short-circuit `Cmd_critcalc` off the *defender* so the crit
//! and its `Random()` draw are both skipped — and
//! [`ability::huge_power_attack`] — Huge Power and Pure Power, doubling the
//! *attacker*'s raw Attack for a physical move before the stat-stage
//! multiply, in the real damage path and the trainer AI's estimate alike.
//! [`ability::guts_attack`] and [`ability::marvel_scale_defense`] read
//! primary status the same way, applied inside
//! [`pokemon::BattlePokemon::attacking_stat`] and
//! [`pokemon::BattlePokemon::defending_stat`] so both damage paths inherit
//! them. [`ability::burn_halves_damage`] feeds the hit pipeline's,
//! the trainer AI estimate's and the forced-replacement selector's
//! `attacker_burned` input (the confusion self-hit still passes `false`): a burned attacker's physical damage halves unless
//! it has Guts (`pokeemerald/src/pokemon.c:3262`-`:3264`), and
//! `Battle::residual_effects` deals the burn tick after bad poison's, which
//! follows ordinary poison
//! ([`status1::burn_residual_damage`], [`status1::bad_poison_tick`]). A burned participant with Water Veil
//! is refused at admission, as a poisoned Immunity holder is, because the
//! move-end cure is unmodelled. The paralysis pipeline still reads Limber and the accuracy
//! abilities ([`paralyze`], `accuracy_check`) and admits a Shed Skin
//! holder: Shed Skin's own end-turn cure draw lives in
//! `Battle::residual_effects`.
//! [`ability::hustle_attack`] raises a Hustle holder's raw physical Attack
//! 150% the same way, before the same stage multiply
//! (`pokeemerald/src/pokemon.c:3205-3206`), so the real damage path, the
//! trainer AI's estimate, and forced-replacement suitability all agree.
//! Compound Eyes and Hustle scale the stage-adjusted accuracy threshold
//! inside [`accuracy::accuracy_check`], as `Cmd_accuracycheck` does, for
//! every path that rolls to hit.
//! [`pokemon::BattlePokemon::ability`] derives the ability from the
//! personality exactly as `CreateBoxMon`/`GetAbilityBySpecies` do, so a
//! seeded party's abilities are deterministic.
//!
//! Two move-slot rules complete the picture. [`pokemon::PpBonuses`] is upstream's packed `ppBonuses` byte —
//! [`pokemon::BattlePokemon`] carries it, [`pokemon::calculate_pp_with_bonus`]
//! is `CalculatePPWithBonus`, and a slot's capacity (what a heal restores to,
//! what PP counts down from) is the PP-Up-adjusted maximum rather than the
//! move's base PP. And the four-known-moves case is not a silent decline:
//! [`pokemon::BattlePokemon::apply_experience`] parks a
//! [`pokemon::PendingMoveLearn`] on the mon itself, which must be answered
//! with a [`pokemon::MoveLearnDecision`] — the mon owns the open question,
//! so a stale copy cannot be replayed and one mon's prompt cannot be
//! answered on another. [`battle::Battle`] surfaces it as
//! [`battle::Battle::pending_move_learn`] /
//! [`battle::Battle::resolve_move_learn`] and enforces it by refusing a
//! turn while the question is open. The pause is faithful to upstream's
//! one-level-at-a-time award loop: the mon holds *at* the prompted level
//! (the award's remainder unconsumed on the token), and everything after
//! the knockout — a trainer's forced send-out, the money payout, the
//! battle's end — waits for the last answer, as upstream finishes the
//! level-up script before `HandleFaintedMonActions`' aftermath. Answering
//! resumes the same walk, so declining still continues to the next learnset
//! entry; replacing a slot clears that slot's PP Ups, exactly as
//! `RemoveMonPPBonus` + `SetMonMoveSlot` do — unless the slot holds an HM
//! move, which is refused the way `IsHMMove2` refuses it
//! ([`error::BattleError::HmMoveCantBeForgotten`]). What this crate still
//! does not own is the *asking* — there is no message layer or summary
//! screen here, and there is deliberately no default answer baked in.
//!
//! Out of scope (see each module's own docs for exactly what is/isn't
//! modelled): the *general* trainer AI beyond the four scripts above and
//! mid-battle switching AI (`I-5`) — the *first* screen some Route 103
//! sight-trainer parties hit, at `battle::trainer_ai::ensure_scoreable` —
//! battle
//! UI/animations, overworld transition, every ability but Overgrow, Liquid
//! Ooze, Battle Armor, Shell Armor, Huge Power, Pure Power, Guts, Marvel
//! Scale, Compound Eyes, Hustle, and Shed Skin (all eleven above), Run
//! Away, Shadow Tag,
//! Arena Trap, and Magnet Pull ([`escape::ensure_admissible`], the
//! wild-battle Run selection gate), Limber ([`paralyze::ParalyzeOutcome::LimberProtected`]),
//! Levitate's Ground-move damage immunity ([`hit::damage_before_roll`] —
//! grounding effects like Gravity and Smack Down are not modelled, so
//! Levitate is otherwise unconditional; see `ensure_admissible` for its
//! Arena Trap immunity), Wonder Guard's block of any powered hit other than
//! Struggle that is not strictly super effective
//! ([`hit::damage_before_roll`], propagated
//! through drain, multi-hit, and fixed damage — Foresight and the
//! two-turn-move distinction upstream's `AttacksThisTurn` check makes are not
//! modelled, so the block is unconditional wherever it applies), and the four
//! stat-drop guards — Clear Body, White Smoke, Keen Eye,
//! Hyper Cutter ([`stat_change`]'s module docs; Shield Dust is the one
//! guard left unmodelled there; [`secondary`] models it for poison, confusion
//! and the secondary Speed drop) —
//! the effect of any held item (a mon carries one, and [`ensure_held_item_playable`]
//! refuses every item that has an effect), every primary status but [`status1::Status1::Paralysed`],
//! [`status1::Status1::Poisoned`], [`status1::Status1::BadlyPoisoned`], [`status1::Status1::Burned`], and
//! [`status1::Status1::Asleep`] (freeze is still unmodelled — see [`status1`]'s module
//! docs; no modelled move inflicts sleep, burn, or bad poison yet, so they arrive only
//! through [`pokemon::BattlePokemon::set_status1`]), every
//! `status2`/`gStatuses3` bit outside [`volatile::Volatiles`]'s four
//! (Substitute chief among them — see [`volatile`]'s module docs), weather,
//! multi/double battles, Mist/Substitute/Safeguard/Protect, and the
//! attacker ability that still reaches an unported chance-doubling for a
//! landed secondary effect — Serene Grace (see
//! [`secondary::ensure_admissible`], which screens the *defender*'s primary
//! status for a poison landing and the *defender*'s Own
//! Tempo and Shield Dust — never a primary status — for a confusion
//! landing) —
//! and the move effects the nine
//! pipelines still do not cover — the secondary-effect trampolines
//! ([`secondary::SECONDARY_TRAMPOLINES`] lists all 31, of which
//! [`secondary::EFFECT_POISON_HIT`], [`secondary::EFFECT_CONFUSE_HIT`] and
//! [`secondary::EFFECT_SPEED_DOWN_HIT`] are the three resolved; the other 28
//! are not, including
//! [`paralyze::EFFECT_PARALYZE`]'s on-hit sibling `EFFECT_PARALYZE_HIT`),
//! `EFFECT_RECOIL` for any move but Struggle (Take Down and
//! Submission stay refused), OHKO, Counter, Bide, Leech Seed and the rest of the end-of-turn
//! residual family, and so on.
//!
//! Paralysis reaches past its own pipeline, so read those two owners before
//! changing turn flow: `battle::Battle::act` cancels a paralysed mover
//! ahead of PP, and [`pokemon::BattlePokemon::speed_for_turn_order`]
//! quarters its Speed.

pub mod ability;
pub mod accuracy;
pub mod battle;
pub mod confuse;
pub mod critical;
pub mod damage;
pub mod defense_curl;
pub mod dex;
pub mod drain;
pub mod error;
pub mod escape;
pub mod exp;
pub mod fixed_damage;
pub mod flag_move;
pub mod gender;
pub mod hit;
mod move_gate;
pub mod multi_hit;
pub mod nature;
pub mod paralyze;
pub mod pokemon;
pub mod secondary;
pub mod stat_change;
pub mod stat_stage;
pub mod status1;
pub mod turn_order;
pub mod volatile;
pub mod wild;

#[cfg(test)]
mod script_rng;

pub use ability::{inverts_drain, pinch_boosts_power, LIQUID_OOZE, OVERGROW};
pub use battle::admission::ensure_participant_admissible;
pub use battle::held_item::ensure_held_item_playable;
pub use battle::trainer::{
    build_trainer_pokemon, ensure_trainer_party_startable, fixed_ivs, roll_non_shiny_ot_id,
    shiny_value, trainer_data, trainer_money, TrainerContext, TrainerPartyMon, SHINY_ODDS,
};
pub use battle::{Battle, BattleEvent, BattleOutcome, PlayerAction, TurnError};
pub use confuse::{is_confuse_effect, resolve_confuse_move, ConfuseOutcome, EFFECT_CONFUSE};
pub use damage::{
    apply_damage_roll, apply_dual_type_effectiveness, apply_stab, apply_type_effectiveness,
    base_damage, calculate_damage, has_stab, BattleRng, DamageInput, MoveCategory, Weather,
    STRUGGLE,
};
pub use defense_curl::{
    is_defense_curl_effect, resolve_defense_curl_move, DefenseCurlOutcome, EFFECT_DEFENSE_CURL,
};
pub use dex::Dex;
pub use drain::{drain_amount, is_drain_effect, resolve_drain, DrainOutcome};
pub use error::BattleError;
pub use exp::{trainer_faint_exp, wild_faint_exp};
pub use fixed_damage::{is_fixed_damage_effect, resolve_fixed_damage_move, FixedDamage};
pub use flag_move::{is_flag_move_effect, resolve_flag_move, FlagMoveOutcome};
pub use gender::Gender;
pub use hit::{accuracy_roll, damage_core, ensure_resolvable, is_ordinary_hit_effect, HitOutcome};
pub use multi_hit::{is_multi_hit_effect, roll_hit_count, MAX_HITS, MIN_HITS};
pub use nature::{Nature, Stat};
pub use paralyze::{is_paralyze_effect, resolve_paralyze_move, ParalyzeOutcome};
pub use pokemon::{
    calculate_pp_with_bonus, compute_stats_with_evs, BattlePokemon, Evs, Ivs, LearnedMove,
    MoveLearnDecision, MoveLearnResolution, MoveSlot, PendingMoveLearn, PpBonuses, StatStages,
    Stats, MAX_IV, MAX_LEVEL, MAX_MON_MOVES, MAX_PP_UPS, MIN_LEVEL, MOVE_NONE, SPECIES_NONE,
    SPECIES_SHEDINJA,
};
pub use secondary::{
    is_confuse_hit_effect, is_poison_hit_effect, is_secondary_effect, is_speed_down_hit_effect,
    spend_effect_chance_draw, SecondaryApplication, Trampoline, EFFECT_CONFUSE_HIT,
    EFFECT_POISON_HIT,
};
pub use stat_change::{
    is_stat_change_effect, stat_change_for_effect, ChangedStat, StatChangeDirection,
    StatChangeEffect, StatChangeMagnitude, StatChangeOutcome, CLEAR_BODY, HYPER_CUTTER, KEEN_EYE,
    WHITE_SMOKE,
};
pub use stat_stage::StatStage;
pub use status1::Status1;
pub use volatile::Volatiles;
pub use wild::{
    build_pokemon_with_random_personality, build_wild_pokemon, ensure_wild_startable,
    initial_moveset, pick_wild_nature,
};
