//! Ordered, caller-visible results from battle turns and move-learning decisions.

use std::error::Error;
use std::fmt;

use assets::{AbilityId, MoveId, SpeciesId};

use crate::error::BattleError;
use crate::stat_change::ChangedStat;
use crate::stat_stage::StatStage;
use crate::status1::Status1;

use super::BattleOutcome;

/// An observable battle result.
///
/// Events are returned in occurrence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BattleEvent {
    /// The player attempted to leave a wild battle.
    ///
    /// This is the first event for a run action. Success is immediately followed
    /// by [`BattleEvent::Ended`] with [`BattleOutcome::PlayerRan`]; after failure,
    /// the opponent may act.
    RunAttempt {
        /// Whether the player made the attempt. Currently always `true`.
        by_player: bool,
        /// Whether the attempt ended the battle.
        success: bool,
    },
    /// A move missed its target.
    Missed {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that missed.
        move_id: MoveId,
    },
    /// A selected move could not execute because its slot had no PP; the turn continues.
    FailedNoPp {
        /// Whether the player selected the move. Currently always `false`.
        by_player: bool,
        /// The move with no PP remaining.
        move_id: MoveId,
    },
    /// A sleeping battler's remaining duration survived this action's
    /// decrement, cancelling its chosen move before any PP was spent
    /// (`CANCELER_ASLEEP`, `src/battle_util.c:2015`-`:2057`).
    ///
    /// This precedes the confusion and paralysis cancellers, which do not
    /// run for the action.
    FastAsleep {
        /// Whether the player's battler is asleep.
        by_player: bool,
        /// The move that was never attempted.
        move_id: MoveId,
    },
    /// A sleeping battler's duration reached zero this action, so it woke
    /// and its chosen move proceeds through the remaining cancellers
    /// (`BattleScript_MoveUsedWokeUp`, `src/battle_util.c:2045`-`:2053`).
    WokeUp {
        /// Whether the player's battler woke.
        by_player: bool,
    },
    /// A confused battler's duration reached zero this action, ending
    /// confusion before the paralysis draw and PP handling run
    /// (`CANCELER_CONFUSED`, `src/battle_util.c:2157`-`:2187`).
    ///
    /// This precedes [`BattleEvent::FullyParalyzed`] when both fire for the
    /// same action.
    SnappedOutOfConfusion {
        /// Whether the player's battler snapped out.
        by_player: bool,
    },
    /// A confusion duration that remained active after this action's
    /// decrement was announced, ahead of the coin draw that decides between
    /// a self-hit and the chosen move (`CANCELER_CONFUSED`'s still-active
    /// branch, `src/battle_util.c:2157`-`:2187`).
    ///
    /// This precedes either [`BattleEvent::ConfusionSelfHit`] or, when the
    /// chosen move proceeds instead, the same action's paralysis draw and PP
    /// handling.
    Confused {
        /// Whether the player's battler is confused.
        by_player: bool,
    },
    /// Confusion's coin draw cancelled the chosen move for a self-hit, after
    /// [`BattleEvent::Confused`] and before any resulting
    /// [`BattleEvent::Fainted`]. No PP is spent, and the paralysis draw does
    /// not run for this action.
    ConfusionSelfHit {
        /// Whether the player's battler hurt itself.
        by_player: bool,
        /// HP removed from that battler, capped at its HP before the hit.
        damage: u32,
    },
    /// A paralysed battler's full-paralysis draw cancelled its chosen move
    /// before any PP was spent.
    FullyParalyzed {
        /// Whether the player's battler was cancelled.
        by_player: bool,
        /// The move that was never attempted.
        move_id: MoveId,
    },
    /// A move connected, but the target's typing made it ineffective, or the
    /// target's Volt Absorb or Water Absorb was already at full HP
    /// (`BattleScript_MonMadeMoveUseless`).
    NoEffect {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that had no effect.
        move_id: MoveId,
    },
    /// `Cmd_typecalc`'s Levitate branch (`battle_script_commands.c:1375-1383`).
    LevitateBlocked {
        /// Whether the player used the move.
        by_player: bool,
        /// The Ground move Levitate blocked.
        move_id: MoveId,
    },
    /// `Cmd_typecalc`'s Wonder Guard branch (`battle_script_commands.c:1409-1418`):
    /// a powered move that was not strictly super effective was blocked.
    WonderGuardBlocked {
        /// Whether the player used the move.
        by_player: bool,
        /// The move Wonder Guard blocked.
        move_id: MoveId,
    },
    /// Volt Absorb or Water Absorb healed the target instead of the move
    /// hitting (`BattleScript_MoveHPDrain`).
    AbilityHealed {
        /// Whether the player used the move.
        by_player: bool,
        /// The move the ability absorbed.
        move_id: MoveId,
        /// HP the target actually regained.
        healed: u32,
    },
    /// Flash Fire absorbed a Fire move (`BattleScript_FlashFireBoost`).
    FlashFireAbsorbed {
        /// Whether the player used the move.
        by_player: bool,
        /// The Fire move Flash Fire absorbed.
        move_id: MoveId,
        /// Whether this activation latched the boost; later ones cancel the
        /// move without boosting (`B_MSG_FLASH_FIRE_NO_BOOST`).
        boosted: bool,
    },
    /// A move dealt damage.
    Hit {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that dealt damage.
        move_id: MoveId,
        /// HP removed from the target, capped at its HP before the hit.
        damage: u32,
        /// Whether the hit was critical.
        is_critical: bool,
    },
    /// Struggle's certain quarter-HP recoil, after its [`BattleEvent::Hit`]
    /// and before either battler's [`BattleEvent::Fainted`]
    /// (`MOVE_EFFECT_RECOIL_25`, `data/battle_scripts_1.s:897-901`,
    /// `:3938-3949`).
    Recoil {
        /// Whether the player used Struggle.
        by_player: bool,
        /// The move that caused the recoil. Currently always [`STRUGGLE`](crate::damage::STRUGGLE).
        move_id: MoveId,
        /// HP removed from the user, capped at its HP before the recoil.
        damage: u32,
    },
    /// A battler's HP reached zero, after the event that caused it.
    Fainted {
        /// Whether the player's battler fainted.
        by_player: bool,
    },
    /// A draining move reported its drain result after its [`BattleEvent::Hit`].
    Drained {
        /// Whether the player used the move.
        by_player: bool,
        /// The draining move.
        move_id: MoveId,
        /// HP restored to the user, capped by its maximum HP and zero at full HP.
        healed: u32,
    },
    /// Liquid Ooze damaged a draining move's user after its [`BattleEvent::Hit`].
    ///
    /// This replaces [`BattleEvent::Drained`] and precedes any resulting
    /// [`BattleEvent::Fainted`] event.
    LiquidOoze {
        /// Whether the player used the draining move.
        by_player: bool,
        /// The draining move.
        move_id: MoveId,
        /// HP removed from the user, capped at its HP before the damage.
        damage: u32,
    },
    /// The number of hits completed by a multi-hit move.
    ///
    /// This follows the move's per-hit [`BattleEvent::Hit`] events and is absent
    /// when the move had no effect.
    MultiHit {
        /// Whether the player used the move.
        by_player: bool,
        /// The multi-hit move.
        move_id: MoveId,
        /// Hits that landed before the loop completed or the target fainted.
        hits: u8,
    },
    /// A move completed with the "nothing happened" result.
    NothingHappened {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that produced the result.
        move_id: MoveId,
    },
    /// A move gave its user the Focus Energy effect.
    GettingPumped {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that applied the effect.
        move_id: MoveId,
    },
    /// A move failed without applying its effect.
    ButItFailed {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that failed.
        move_id: MoveId,
    },
    /// A move started or refreshed its user's Charge effect.
    ChargingPower {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that applied the effect.
        move_id: MoveId,
    },
    /// The wild opponent fled, immediately before the battle ends.
    WildFled,
    /// A move lowered its target's stat stage.
    StatFell {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that lowered the stat.
        move_id: MoveId,
        /// The target stat.
        stat: ChangedStat,
        /// The target's resulting stage.
        new_stage: StatStage,
        /// The requested drop, which may exceed the actual clamped change.
        magnitude: u8,
    },
    /// A move could not lower a target stat already at its minimum stage.
    StatWontGoLower {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that targeted the stat.
        move_id: MoveId,
        /// The target stat.
        stat: ChangedStat,
    },
    /// A target's ability prevented a move from lowering its stat stage.
    StatLossPrevented {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that targeted the stat.
        move_id: MoveId,
        /// The target stat.
        stat: ChangedStat,
        /// The ability that prevented the change.
        ability: AbilityId,
    },
    /// A sound move exited through `BattleScript_SoundproofProtected`
    /// (`data/battle_scripts_1.s:4158`-`:4164`): the target's
    /// [`AbilityId::SOUNDPROOF`] blocked the move itself, not a named stat
    /// drop.
    SoundproofProtected {
        /// Whether the player used the move.
        by_player: bool,
        /// The move Soundproof blocked.
        move_id: MoveId,
    },
    /// A move raised its user's stat stage.
    StatRose {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that raised the stat.
        move_id: MoveId,
        /// The user's affected stat.
        stat: ChangedStat,
        /// The user's resulting stage.
        new_stage: StatStage,
        /// The requested rise, which may exceed the actual clamped change.
        magnitude: u8,
    },
    /// A move could not raise a user stat already at its maximum stage.
    StatWontGoHigher {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that targeted the stat.
        move_id: MoveId,
        /// The user's affected stat.
        stat: ChangedStat,
    },
    /// A move inflicted [`crate::status1::Status1::Paralysed`] on its target.
    Paralyzed {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that inflicted paralysis.
        move_id: MoveId,
    },
    /// A move's target already carried [`crate::status1::Status1::Paralysed`],
    /// so no accuracy draw occurred.
    AlreadyParalyzed {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that targeted the already-paralysed battler.
        move_id: MoveId,
    },
    /// A paralysis move exited through `BattleScript_LimberProtected`
    /// (`data/battle_scripts_1.s:1034`-`:1038`): the target's
    /// [`AbilityId::LIMBER`] blocked the move before `typecalc` or the
    /// accuracy draw.
    LimberProtected {
        /// Whether the player used the move.
        by_player: bool,
        /// The move the ability blocked.
        move_id: MoveId,
    },
    /// A [`BattleEvent::Paralyzed`] target's Synchronize reflected the status
    /// at move end, and the original attacker's [`AbilityId::LIMBER`] blocked
    /// the reflection
    /// (`src/battle_util.c:2971`-`:2984`, `src/battle_script_commands.c:2395`-`:2415`).
    ///
    /// This follows the [`BattleEvent::Paralyzed`] event it reflects.
    SynchronizeLimberProtected {
        /// Whether the player used the paralysing move.
        by_player: bool,
        /// The move whose paralysis was reflected.
        move_id: MoveId,
    },
    /// A [`BattleEvent::Paralyzed`] target's Synchronize reflected the status
    /// back onto the original attacker at move end
    /// (`MOVEEND_SYNCHRONIZE_TARGET`, `src/battle_util.c:2971`-`:2984`).
    ///
    /// This follows the [`BattleEvent::Paralyzed`] event it reflects and
    /// consumes no RNG: the reflection re-enters `SetMoveEffect` without
    /// `typecalc` or `accuracycheck`.
    ParalyzedBySynchronize {
        /// Whether the player used the paralysing move.
        by_player: bool,
        /// The move whose paralysis was reflected.
        move_id: MoveId,
    },
    /// `STRINGID_PKMNWASPOISONED` (`data/battle_scripts_1.s:319-321`).
    Poisoned {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that inflicted poison.
        move_id: MoveId,
    },
    /// A [`BattleEvent::Poisoned`] target's Synchronize reflected the status
    /// back onto the original attacker after the target's own faint settlement
    /// (`MOVEEND_SYNCHRONIZE_TARGET`, `battle_util.c:2971`-`:2986`;
    /// `data/battle_scripts_1.s:265`-`:267`).
    ///
    /// This follows the [`BattleEvent::Poisoned`] event it reflects and
    /// consumes no RNG: the reflection re-enters `SetMoveEffect` without
    /// `typecalc` or `accuracycheck`.
    PoisonedBySynchronize {
        /// Whether the player used the poisoning move.
        by_player: bool,
        /// The move whose poison was reflected.
        move_id: MoveId,
    },
    /// The target's [`AbilityId::STATIC`] paralysed the attacker after a
    /// damaging contact hit (`MOVEEND_ON_DAMAGE_ABILITIES`,
    /// `battle_script_commands.c:4280`; `ABILITY_STATIC`,
    /// `battle_util.c:2821`-`:2835`).
    ///
    /// The roll is one `Random() % 3` draw taken after the target's faint
    /// settlement, whether or not the holder survived the hit; only a zero
    /// roll on a healthy attacker emits this event.
    ParalyzedByStatic {
        /// Whether the player used the contact move and so was paralysed.
        by_player: bool,
        /// The contact move that triggered Static.
        move_id: MoveId,
    },
    /// The target's [`AbilityId::ROUGH_SKIN`] hurt the attacker after a
    /// damaging contact hit (`MOVEEND_ON_DAMAGE_ABILITIES`,
    /// `battle_script_commands.c:4280`; `ABILITY_ROUGH_SKIN`,
    /// `battle_util.c:2767`-`:2780`; `BattleScript_RoughSkinActivates`,
    /// `data/battle_scripts_1.s:4184`).
    ///
    /// The damage is a sixteenth of the attacker's maximum HP, at least one and
    /// at most its remaining HP, whether or not the holder survived the hit.
    /// No RNG is consumed. A [`BattleEvent::Fainted`] for the attacker follows
    /// when the damage knocks it out.
    RoughSkinDamage {
        /// Whether the player used the contact move and so lost the HP.
        by_player: bool,
        /// The contact move that triggered Rough Skin.
        move_id: MoveId,
        /// HP the attacker actually lost.
        damage: u32,
    },
    /// A contact ability's paralysis on the attacker (a
    /// [`BattleEvent::ParalyzedByStatic`] or [`BattleEvent::ParalyzedByEffectSpore`])
    /// was reflected by the attacker's own Synchronize onto the surviving
    /// holder (`MOVEEND_SYNCHRONIZE_ATTACKER`, `battle_script_commands.c:4291`-
    /// `:4294`; `ABILITYEFFECT_ATK_SYNCHRONIZE`, `battle_util.c:2987`-`:3001`).
    ///
    /// This follows the contact ability's own paralysis event and consumes no RNG.
    ParalyzedByAttackerSynchronize {
        /// Whether the player used the contact move; the opposing holder is
        /// the battler paralysed.
        by_player: bool,
        /// The contact move that triggered the holder's ability.
        move_id: MoveId,
    },
    /// The target's [`AbilityId::EFFECT_SPORE`] put the attacker to sleep after a
    /// damaging contact hit (`MOVEEND_ON_DAMAGE_ABILITIES`,
    /// `battle_script_commands.c:4280`; `ABILITY_EFFECT_SPORE`,
    /// `battle_util.c:2782`-`:2804`; `STATUS1_SLEEP`,
    /// `battle_script_commands.c:2273`-`:2298`, `:2480`-`:2487`).
    ///
    /// Spent draws, in order: the `Random() % 10` chance, each `Random() & 3`
    /// selector until a nonzero one picks sleep, then the sleep duration;
    /// only an attacker with no primary status and neither Insomnia nor Vital
    /// Spirit reaches the duration draw and this event. A holder that
    /// fainted to the hit still triggers it, after its [`BattleEvent::Fainted`].
    AsleepByEffectSpore {
        /// Whether the player used the contact move and so fell asleep.
        by_player: bool,
        /// The contact move that triggered Effect Spore.
        move_id: MoveId,
    },
    /// The target's [`AbilityId::EFFECT_SPORE`] poisoned the attacker after a
    /// damaging contact hit (`ABILITY_EFFECT_SPORE`, `battle_util.c:2782`-
    /// `:2804`; `STATUS1_POISON`, `battle_script_commands.c:2299`-`:2340`).
    ///
    /// Spent draws, in order: the `Random() % 10` chance, then each
    /// `Random() & 3` selector until a nonzero one picks poison. A Poison or
    /// Steel type, an existing primary status and Immunity keep the attacker
    /// unchanged and emit nothing.
    PoisonedByEffectSpore {
        /// Whether the player used the contact move and so was poisoned.
        by_player: bool,
        /// The contact move that triggered Effect Spore.
        move_id: MoveId,
    },
    /// The target's [`AbilityId::EFFECT_SPORE`] paralysed the attacker after a
    /// damaging contact hit (`ABILITY_EFFECT_SPORE`, `battle_util.c:2782`-
    /// `:2804`; `STATUS1_PARALYSIS`, `battle_script_commands.c:2395`-`:2424`).
    ///
    /// Spent draws, in order: the `Random() % 10` chance, then each
    /// `Random() & 3` selector until a nonzero one picks the burn effect
    /// upstream replaces with paralysis. Limber and an existing primary
    /// status keep the attacker unchanged and emit nothing.
    ParalyzedByEffectSpore {
        /// Whether the player used the contact move and so was paralysed.
        by_player: bool,
        /// The contact move that triggered Effect Spore.
        move_id: MoveId,
    },
    /// The target's [`AbilityId::POISON_POINT`] poisoned the attacker after a
    /// damaging contact hit (`MOVEEND_ON_DAMAGE_ABILITIES`,
    /// `battle_script_commands.c:4280`; `ABILITY_POISON_POINT`,
    /// `battle_util.c:2806`-`:2818`).
    ///
    /// The roll is one `Random() % 3` draw taken after the target's faint
    /// settlement, whether or not the holder survived the hit; only a zero
    /// roll on an attacker that the poison guards allow emits this event.
    PoisonedByPoisonPoint {
        /// Whether the player used the contact move and so was poisoned.
        by_player: bool,
        /// The contact move that triggered Poison Point.
        move_id: MoveId,
    },
    /// A contact ability's poison on the attacker (a
    /// [`BattleEvent::PoisonedByPoisonPoint`] or [`BattleEvent::PoisonedByEffectSpore`])
    /// was reflected by the attacker's own Synchronize onto the surviving
    /// holder (`MOVEEND_SYNCHRONIZE_ATTACKER`, `battle_script_commands.c:4291`-
    /// `:4294`; `ABILITYEFFECT_ATK_SYNCHRONIZE`, `battle_util.c:2987`-`:3001`).
    ///
    /// This follows the contact ability's own poison event and consumes no RNG.
    PoisonedByAttackerSynchronize {
        /// Whether the player used the contact move; the opposing holder is
        /// the battler poisoned.
        by_player: bool,
        /// The contact move that triggered the holder's ability.
        move_id: MoveId,
    },
    /// An attacker's Synchronize reflected a contact ability's poison (see
    /// [`BattleEvent::PoisonedByAttackerSynchronize`]), and the holder's own
    /// Poison or Steel typing protected it (`BattleScript_PSNPrevention`,
    /// `battle_script_commands.c:2320`-`:2329`).
    ///
    /// This follows the contact ability's own poison event.
    AttackerSynchronizePoisonOrSteelTypeProtected {
        /// Whether the player used the contact move; the opposing holder was
        /// the protected battler.
        by_player: bool,
        /// The contact move that triggered Poison Point.
        move_id: MoveId,
    },
    /// A poison reflection's original attacker was protected by its own
    /// [`AbilityId::IMMUNITY`] (`BattleScript_PSNPrevention`,
    /// `battle_script_commands.c:2300`-`:2319`).
    ///
    /// This follows the [`BattleEvent::Poisoned`] event it reflects.
    SynchronizeImmunityProtected {
        /// Whether the player used the poisoning move.
        by_player: bool,
        /// The move whose poison was reflected.
        move_id: MoveId,
    },
    /// A poison reflection's original attacker was protected by its own
    /// Poison or Steel typing (`BattleScript_PSNPrevention`,
    /// `battle_script_commands.c:2320`-`:2329`).
    ///
    /// This follows the [`BattleEvent::Poisoned`] event it reflects.
    SynchronizePoisonOrSteelTypeProtected {
        /// Whether the player used the poisoning move.
        by_player: bool,
        /// The move whose poison was reflected.
        move_id: MoveId,
    },
    /// A move inflicted a fresh confusion volatile on its target, whether
    /// directly (`BattleScript_EffectConfuse`'s own `seteffectprimary`,
    /// `data/battle_scripts_1.s:918-922`) or through a damaging hit's
    /// trailing chance draw (`EFFECT_CONFUSE_HIT`'s trampoline into the same
    /// `SetMoveEffect` `MOVE_EFFECT_CONFUSION` case,
    /// `src/battle_script_commands.c:2528`-`:2544`).
    ///
    /// This is distinct from [`BattleEvent::Confused`], which announces an
    /// *already*-active duration ahead of that action's own coin draw rather
    /// than a fresh application.
    ConfusionInflicted {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that inflicted confusion.
        move_id: MoveId,
    },
    /// A direct confuse move's target already carried an active confusion
    /// volatile, so no accuracy draw occurred
    /// (`BattleScript_AlreadyConfused`, `data/battle_scripts_1.s:918`-`:922`).
    ///
    /// An on-hit trampoline's own already-confused defender instead lands
    /// silently: the hit still connects, but nothing beyond its own
    /// [`BattleEvent::Hit`] is reported, matching an already-poisoned
    /// defender's identical silent no-land.
    AlreadyConfused {
        /// Whether the player used the move.
        by_player: bool,
        /// The move that targeted the already-confused battler.
        move_id: MoveId,
    },
    /// A direct confuse move exited through `BattleScript_OwnTempoPrevents`
    /// (`data/battle_scripts_1.s:4152`-`:4156`): the target's
    /// [`AbilityId::OWN_TEMPO`] blocked the move before the accuracy draw.
    ///
    /// An on-hit trampoline's own Own-Tempo defender instead lands silently,
    /// the same way an already-confused trampoline target does; see
    /// [`BattleEvent::AlreadyConfused`]'s docs.
    OwnTempoProtected {
        /// Whether the player used the move.
        by_player: bool,
        /// The move the ability blocked.
        move_id: MoveId,
    },
    /// `STRINGID_PKMNHURTBYPOISON` (`data/battle_scripts_1.s:3736-3737`).
    HurtByPoison {
        /// Whether the player's battler was hurt.
        by_player: bool,
        /// HP removed, capped at the battler's HP before the tick.
        damage: u32,
    },
    /// The same poison-turn script for `ENDTURN_BAD_POISON`, which runs after
    /// ordinary poison and before burn (`pokeemerald/src/battle_util.c:1533`-
    /// `:1546`). Damage is `max(1, max HP / 16)` times the counter after this
    /// tick's increment.
    HurtByBadPoison {
        /// Whether the player's battler was hurt.
        by_player: bool,
        /// HP removed, capped at the battler's HP before the tick.
        damage: u32,
    },
    /// `STRINGID_PKMNHURTBYBURN` (`data/battle_scripts_1.s:3750`-`:3753`): a
    /// burned battler's `ENDTURN_BURN` tick, which follows its
    /// [`BattleEvent::HurtByPoison`] slot (`pokeemerald/src/battle_util.c:1550`-
    /// `:1560`).
    HurtByBurn {
        /// Whether the player's battler was hurt.
        by_player: bool,
        /// HP removed, capped at the battler's HP before the tick.
        damage: u32,
    },
    /// `STRINGID_PKMNSXCUREDYPROBLEM`, `BattleScript_ShedSkinActivates`
    /// (`data/battle_scripts_1.s:3993-3997`): a living, statused Shed Skin
    /// holder's end-turn draw cured its primary status, before that
    /// battler's own [`BattleEvent::HurtByPoison`],
    /// [`BattleEvent::HurtByBadPoison`], or
    /// [`BattleEvent::HurtByBurn`] tick would otherwise run
    /// (`ABILITYEFFECT_ENDTURN` precedes `ENDTURN_POISON` and `ENDTURN_BURN`,
    /// `pokeemerald/src/battle_util.c:1494`-`:1560`).
    ShedSkinCured {
        /// Whether the player's battler was cured.
        by_player: bool,
        /// The primary status Shed Skin cured.
        status: Status1,
    },
    /// A trainer sent out a party member: the forced replacement after a
    /// faint, or a prechosen voluntary switch.
    TrainerSentOut {
        /// The replacement's species.
        species: SpeciesId,
        /// Non-fainted party members behind the replacement.
        bench_remaining: usize,
    },
    /// The player sent out the first non-fainted reserve after an active
    /// faint, under the headless party-order policy (no player choice, no
    /// party-screen UI). Reported for a wild or trainer battle alike.
    PlayerSentOut {
        /// The replacement's species.
        species: SpeciesId,
        /// Usable (non-fainted) reserves remaining after the replacement.
        reserves_remaining: usize,
    },
    /// The full experience award after the opposing battler fainted.
    ///
    /// Application begins before this event. If the award pauses at a
    /// move-learning decision, a following [`BattleEvent::MoveLearnPrompt`]
    /// identifies it; the unconsumed remainder is applied when learning resumes.
    ExpGained(u32),
    /// Move learning paused experience application and faint resolution.
    ///
    /// This follows [`BattleEvent::ExpGained`] or a previous move-learning answer
    /// and blocks further turns until the player answers it.
    MoveLearnPrompt {
        /// The move offered to the player.
        move_id: MoveId,
    },
    /// The first event returned after the player replaces a known move.
    MoveReplaced {
        /// The newly learned move.
        learned: MoveId,
        /// The move removed from the moveset.
        forgotten: MoveId,
        /// The moveset slot that was replaced.
        slot: usize,
    },
    /// The first event returned after the player declines an offered move.
    MoveLearnDeclined {
        /// The declined move.
        move_id: MoveId,
    },
    /// Prize money owed to the caller after a trainer victory.
    ///
    /// [`super::Battle`] does not apply this amount to save data. This event
    /// immediately precedes [`BattleEvent::Ended`] with
    /// [`BattleOutcome::PlayerWon`].
    MoneyGained(u32),
    /// The battle reached its terminal outcome and accepts no further turns.
    ///
    /// This is the final event returned for the battle.
    Ended(BattleOutcome),
}

/// A turn failure together with the observable events committed before it.
///
/// Battle state and randomness are not rolled back. [`TurnError::events`]
/// retains events in occurrence order. An empty slice usually identifies a
/// call rejected before the turn began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnError {
    pub(super) events: Vec<BattleEvent>,
    pub(super) error: BattleError,
}

impl TurnError {
    /// Returns the error that stopped the turn.
    #[must_use]
    pub const fn error(&self) -> BattleError {
        self.error
    }

    /// Borrows the events committed before the turn stopped, in occurrence order.
    ///
    /// An empty slice does not prove that battle state or randomness is unchanged.
    #[must_use]
    pub fn events(&self) -> &[BattleEvent] {
        &self.events
    }

    /// Returns the committed events, consuming the turn error.
    #[must_use]
    pub fn into_events(self) -> Vec<BattleEvent> {
        self.events
    }
}

impl From<BattleError> for TurnError {
    fn from(error: BattleError) -> Self {
        Self {
            events: Vec::new(),
            error,
        }
    }
}

impl fmt::Display for TurnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (after {} event(s))", self.error, self.events.len())
    }
}

impl Error for TurnError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}
