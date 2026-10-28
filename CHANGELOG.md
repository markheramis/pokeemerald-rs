# Changelog

Concise patch notes for development versions, carried from `dev` through the release channels. Dates record when changes entered `dev`; a version listed here is not necessarily a published release. [GitHub Releases](https://github.com/LunchBox951/pokeemerald-rs/releases) records published builds and their artifacts.

[`RELEASE.md` §Changelog](RELEASE.md#changelog) owns the update policy and entry format. Versions follow the repository's `FINAL.MAJOR.MINOR.PATCH` scheme.

## [0.0.79.0] - 2026-10-28

- Opposing trainers with another healthy Pokémon may switch out before choosing a move.

## [0.0.78.0] - 2026-10-27

- The Bag now shows a TM or HM's move type, power, accuracy and PP while its menu is open.

## [0.0.77.0] - 2026-10-27

- General improvements in preparation for battle move animations.

## [0.0.76.0] - 2026-10-26

- General improvements in preparation for text that switches fonts mid-line.

## [0.0.75.0] - 2026-10-26

- General improvements in preparation for trainers using items in battle.

## [0.0.74.0] - 2026-10-26

- Mom now turns to face you when you start her dialogue.
- Corrected the loudness of the Game Boy sound channels against sampled instruments.
- General improvements to how your lead Pokémon is tracked between the field and battle.

## [0.0.73.0] - 2026-10-26

- General improvements in preparation for Pokémon cries extracted from your ROM.

## [0.0.72.0] - 2026-10-26

- General improvements in preparation for button icons in dialogue.

## [0.0.71.0] - 2026-10-24

- Trainers who spot you on Route 103 now walk up and challenge you before the battle starts.

## [0.0.70.0] - 2026-10-24

- General improvements in preparation for map characters that move and turn.

## [0.0.69.0] - 2026-10-24

- General improvements in preparation for the Bag screen in the field and in battle.

## [0.0.68.0] - 2026-10-23

- Improved which interaction is chosen when an NPC, sign, or map tile is in front of you.
- Added battle support for sleep, and improved wild encounters affected by Cute Charm and Synchronize.
- Various save, graphics, and audio bugs fixed.

## [0.0.67.0] - 2026-10-22

- General improvements in preparation for Brendan and May's opening portraits.
- Improved save-file handling on Windows.

## [0.0.66.0] - 2026-10-22

- Added support for importing starter-selection graphics from your ROM.
- Improved party handling after losing a battle.
- General improvements in preparation for opening scenes and map characters.

## [0.0.65.0] - 2026-10-21

- Improved how opposing trainers evaluate stat-changing moves.
- Various dialogue spacing, save, and audio bugs fixed.

## [0.0.64.0] - 2026-10-20

- General improvements in preparation for importing battle animations from your ROM.
- Improved audio resampling.

## [0.0.63.0] - 2026-10-20

- General improvements in preparation for starter-selection graphics.
- Input now respects the saved button mode.
- Various menu, battle, audio, and save bugs fixed.

## [0.0.62.0] - 2026-10-19

- Improved how songs share music playback.
- Added support for more weather-related ability effects when Pokémon enter battle.
- Fixed a blank-window problem on Wayland and various options, movement, and graphics bugs.

## [0.0.61.0] - 2026-10-19

- General improvements to preparing game data from the supported game version.

## [0.0.60.0] - 2026-10-19

- The opening screen now updates throughout a fade.
- Improved Serene Grace's effect on confusion chances.

## [0.0.59.0] - 2026-10-18

- General improvements in preparation for Pokémon cries.

## [0.0.58.0] - 2026-10-18

- General improvements in preparation for changing game options.
- Improved contact abilities, Plus and Minus, and Deoxys's Speed Forme.
- Various opening-scene, movement, and audio bugs fixed.

## [0.0.57.0] - 2026-10-17

- General improvements in preparation for displaying starter Pokémon in battle.

## [0.0.56.0] - 2026-10-17

- General improvements in preparation for importing Bag graphics from your ROM.

## [0.0.55.0] - 2026-10-17

- General improvements in preparation for selecting items from the Bag.

## [0.0.54.0] - 2026-10-17

- General improvements in preparation for entering names.
- Improved Rough Skin and several abilities that take effect when a Pokémon enters battle.
- Various text, timing, and trainer-decision bugs fixed.

## [0.0.53.0] - 2026-10-16

- General improvements in preparation for battle animations and Bag graphics.

## [0.0.52.0] - 2026-10-16

- General improvements in preparation for battle text boxes and menus.

## [0.0.51.0] - 2026-10-16

- Added battle support for Volt Absorb, Water Absorb, and Flash Fire.

## [0.0.50.0] - 2026-10-16

- Improved how opposing trainers evaluate status moves.
- Improved Truant and Intimidate behaviour, with various audio and graphics bugs fixed.

## [0.0.49.0] - 2026-10-15

- Added support for forced slipping on map tiles.
- Improved confusion, contact abilities, Thick Fat, trainer decisions, and several other battle rules.
- Various menu, movement, save, graphics, audio, and ROM import bugs fixed.

## [0.0.48.0] - 2026-10-07

- Added the movement sequence for bumping into a wall.
- Improved fades that affect selected groups of colours.
- Various save and general bugs fixed.

## [0.0.47.0] - 2026-10-06

- Hold B to run where running is permitted.
- Various NPC interaction, save, and audio bugs fixed.

## [0.0.46.0] - 2026-10-05

- New games now retain the selected player gender.
- Improved interactions with map tiles and NPC placement after continuing a save.
- Various movement, battle, audio, and save bugs fixed.

## [0.0.45.0] - 2026-10-02

- Improved save storage to read and write all fourteen sections of the game's save data.

## [0.0.44.0] - 2026-10-01

- Continuing certain saves now uses their recorded return location.
- Fixed overlapping sprite order, saved player elevation, and trainer alert timing.

## [0.0.43.0] - 2026-09-30

- Added support for forced walking and sliding across map tiles.
- Improved NPC state during map visits and how fade colours combine on screen.

## [0.0.42.0] - 2026-09-29

- Added confusion from moves and their secondary effects.
- Fixed message-box colours and various audio and import bugs.

## [0.0.41.0] - 2026-09-28

- Added support for sending out a usable reserve Pokémon after the active Pokémon faints in a trainer battle.
- Improved battles where both sides use a move.

## [0.0.40.0] - 2026-09-28

- Opening transitions now wait for their fade to finish before advancing.

## [0.0.39.0] - 2026-09-28

- Confused Pokémon can now hurt themselves.
- Trainer dialogue now respects the saved text-speed option.

## [0.0.38.0] - 2026-09-27

- Added support for a Pokémon's confusion state in battle.
- Dialogue now respects the saved text-speed option.
- Various NPC, text, save, audio, and display bugs fixed.

## [0.0.37.0] - 2026-09-26

- Added support for the game's palette fades.

## [0.0.36.0] - 2026-09-26

- Improved music playback and the sound at the end of samples.
- Improved wild encounters involving trapping abilities.
- Various import and general bugs fixed.

## [0.0.35.0] - 2026-09-25

- Synchronize can now reflect poisoning.
- Various movement, visual effects, and import bugs fixed.

## [0.0.34.0] - 2026-09-24

- Added support for sending out a usable reserve Pokémon after the active Pokémon faints in a wild battle.

## [0.0.33.0] - 2026-09-24

- Shed Skin can now cure a Pokémon's status at the end of a turn.

## [0.0.32.0] - 2026-09-24

- Improved audio playback timing and the handling of music as it finishes.
- New games preserve existing options, and save confirmation better matches the original timing.
- Various battle, text, display, and ROM import bugs fixed.

## [0.0.31.0] - 2026-09-20

- Added downloadable nightly builds for Linux and Windows.
- More abilities now affect battles, including Levitate, Wonder Guard, Pressure, Guts, Marvel Scale, Compound Eyes, Hustle, and Synchronize.
- Various movement, trainer, save, and audio bugs fixed.

## [0.0.30.0] - 2026-09-11

- Added poisoning from move effects and poison damage at the end of a turn.
- Improved the resting camera position and window redraws.
- Various save, menu, audio, and display bugs fixed.

## [0.0.29.0] - 2026-09-08

- Import a supported Emerald ROM to prepare the game's assets locally.
- Continuing a save now sends out the first usable party member.
- Various dialogue, save, battle, and audio bugs fixed.

## [0.0.28.0] - 2026-09-06

- Paralysis now affects battle turns.
- Improved Pokémon stat growth after battles, and corrected Shedinja's HP.
- Various save, text, audio, and display bugs fixed.

## [0.0.27.0] - 2026-08-26

- Improved the sequence when a trainer spots and approaches you.
- Various battle, save, dialogue, colour, and audio bugs fixed.

## [0.0.26.0] - 2026-08-21

- Pokémon retain PP Up bonuses when learning moves, and a full moveset can prompt for a replacement.

## [0.0.25.0] - 2026-08-21

- Added battle support for draining, fixed-damage, and multi-hit moves.

## [0.0.24.0] - 2026-08-20

- More stat-changing moves and Clear Body now follow the original battle rules.
- Improved which sprites remain visible in crowded scenes.

## [0.0.23.0] - 2026-08-19

- Improved music when too many notes compete to play at once.
- The in-game menu remembers its cursor position, and trainer wins award prize money.

## [0.0.22.0] - 2026-08-17

- Clarified that the planned full release covers the complete single-player game. Gameplay is unchanged.

## [0.0.21.0] - 2026-08-16

- Added early support for trainers spotting you on Route 103; their battle handoff remains incomplete.

## [0.0.20.0] - 2026-08-16

- The first rescue battle now leads into healing, story progress, and the return to Professor Birch's lab.

## [0.0.19.0] - 2026-08-16

- NPC positions in Oldale Town now respond to story progress.
- Various audio and display bugs fixed.

## [0.0.18.0] - 2026-08-15

- Losing a battle now heals your party, halves your money, and returns you home.

## [0.0.17.0] - 2026-08-15

- Added background NPCs in Oldale Town and on Route 103.

## [0.0.16.0] - 2026-08-14

- See neighbouring map tiles across connected map edges.

## [0.0.15.0] - 2026-08-14

- Added support for learning moves when a Pokémon gains a level in battle.

## [0.0.14.0] - 2026-08-14

- Reach the rival battle on Route 103 through the overworld.
- Fixed the in-game menu's colours and Route 101 progress after continuing a save.

## [0.0.13.0] - 2026-08-12

- Added support for the rival's early trainer-battle rules and team.

## [0.0.12.0] - 2026-08-12

- General improvements to checks covering the journey from startup to the first battle.

## [0.0.11.0] - 2026-08-11

- Title-screen music now plays continuously, with improved reverb.

## [0.0.10.0] - 2026-08-11

- Save from the in-game menu, preserving your party and the direction you were facing.

## [0.0.9.0] - 2026-08-10

- General improvements to automated checks for visual and gameplay errors.

## [0.0.8.0] - 2026-08-10

- Save progress to disk and resume it with Continue from the main menu.

## [0.0.7.0] - 2026-08-09

- Entering Route 101 can now trigger Professor Birch's rescue and the first battle.

## [0.0.6.0] - 2026-08-09

- General improvements to checks for visual errors on the title screen and main menu.

## [0.0.5.0] - 2026-08-08

- Added support for constructing and running the scripted first battle.

## [0.0.4.0] - 2026-08-07

- Improved movement around the bedroom bed and tiles that block movement from particular directions.

## [0.0.3.0] - 2026-08-07

- Improved the special battle rules for the first Route 101 encounter.

## [0.0.2.0] - 2026-08-07

- The new-game menu more closely matches Emerald.
- Fixed NPCs drifting out of place while the camera moves.

## [0.0.1.0] - 2026-08-06

- Encounter wild Pokémon while walking through grass and play an early wild battle.
- Walk between Littleroot Town and Route 101, with animated map tiles and arrow warps.

## [0.0.0.0] - 2026-07-31

- Initial prerelease establishing the repository release path, with an early title screen, new-game introduction, and exploration of the opening area.

[0.0.79.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/d828e8ae72b532a2de0148cbf62942cd0805b7ec...dev
[0.0.78.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/7bb89a891a17166542bca78a24db79325000dae9...dev
[0.0.77.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/1bd4a0c1485db83589e2cf761f83c6c9cd37c2c6...7bb89a891a17166542bca78a24db79325000dae9
[0.0.76.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/42b847579ee1ed8368ccf8fb64f932a7c9bd82f3...1bd4a0c1485db83589e2cf761f83c6c9cd37c2c6
[0.0.75.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/36af396e97b06960838320cc158b442431b44783...42b847579ee1ed8368ccf8fb64f932a7c9bd82f3
[0.0.74.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/98ba10237970a6e9de877ced16b1b223190090f0...36af396e97b06960838320cc158b442431b44783
[0.0.73.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/8cee32b52a7ce45be4aa91b373848039a0cccd6d...98ba10237970a6e9de877ced16b1b223190090f0
[0.0.72.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/dd1f5200aa09f2fd14fa5aa18b4dbd0c4a03fb3e...8cee32b52a7ce45be4aa91b373848039a0cccd6d
[0.0.71.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/ce0fd44c695f1ac523fc958f4401607339397c5b...dd1f5200aa09f2fd14fa5aa18b4dbd0c4a03fb3e
[0.0.70.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/ddd60e3efb8065b0d2a4606dbf63545b51174497...dev
[0.0.69.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/eb737282b387d993825d6b0582e39de77b682582...ddd60e3efb8065b0d2a4606dbf63545b51174497
[0.0.68.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/974ece58d7d8a70dbaa7afe7f1b00923037faa59...eb737282b387d993825d6b0582e39de77b682582
[0.0.67.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/f028fc93e4a2205470d2f46150abdc28d4bd5428...974ece58d7d8a70dbaa7afe7f1b00923037faa59
[0.0.66.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/ce8db9b88a3ce8b0357ad2954ed8f7cbe9a2b3ea...f028fc93e4a2205470d2f46150abdc28d4bd5428
[0.0.65.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/f57b070753e6b4f06431c7c2bef45957888ffd53...ce8db9b88a3ce8b0357ad2954ed8f7cbe9a2b3ea
[0.0.64.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/251a20722615c9b6f7c1dcff9ddb421b8ce5dc92...f57b070753e6b4f06431c7c2bef45957888ffd53
[0.0.63.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/b24027d38bd8c752b728246c7643de43c23ab154...251a20722615c9b6f7c1dcff9ddb421b8ce5dc92
[0.0.62.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/f832883ac3313e67be92e401278114074b4552df...b24027d38bd8c752b728246c7643de43c23ab154
[0.0.61.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/21e809ea3da6486f9b38e6e52213e3bca7dadb5e...f832883ac3313e67be92e401278114074b4552df
[0.0.60.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/b08eea15c330b4e899d0ca81263221f914acd33f...21e809ea3da6486f9b38e6e52213e3bca7dadb5e
[0.0.59.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/5bd2728a39f3f391edd44c6c120e5c85de428e72...b08eea15c330b4e899d0ca81263221f914acd33f
[0.0.58.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/878398fe22ddc7a2c6581c8e7c0f1f4f062baaaf...5bd2728a39f3f391edd44c6c120e5c85de428e72
[0.0.57.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/dbf258356e7d3ddc20b1617387f389c84ca94fad...878398fe22ddc7a2c6581c8e7c0f1f4f062baaaf
[0.0.56.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/d423609f2bf55105951b85d2c46721a1a1483083...dbf258356e7d3ddc20b1617387f389c84ca94fad
[0.0.55.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/06c884a07aa53228cc6926f021f1f4f2b17a7a43...d423609f2bf55105951b85d2c46721a1a1483083
[0.0.54.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/5064d9c7290ad479cb9054deeb42d499ca57e1cc...06c884a07aa53228cc6926f021f1f4f2b17a7a43
[0.0.53.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/fdf4ea545674143c04a9ebed0d79b1f02933c27a...5064d9c7290ad479cb9054deeb42d499ca57e1cc
[0.0.52.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/a836cad3fc8394d03d527a91b40c1b85f11f97d9...fdf4ea545674143c04a9ebed0d79b1f02933c27a
[0.0.51.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/f5d2edf5dc7825095c60dcb6a5579e67574ab7a8...a836cad3fc8394d03d527a91b40c1b85f11f97d9
[0.0.50.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/26348b3725c5568e8b257439345423de98a83890...f5d2edf5dc7825095c60dcb6a5579e67574ab7a8
[0.0.49.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/48b80ec95d753fc7b0d8dcec776370394ea079d6...26348b3725c5568e8b257439345423de98a83890
[0.0.48.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/f9c8c069007ef3225fb38456380338ea16f96caf...48b80ec95d753fc7b0d8dcec776370394ea079d6
[0.0.47.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/8609769394c05ed0392c37c2f72b23d163e3a431...f9c8c069007ef3225fb38456380338ea16f96caf
[0.0.46.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/1f7746d5ddea925ffb8fa52e8abb93f78eae8002...8609769394c05ed0392c37c2f72b23d163e3a431
[0.0.45.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/80cc4faa023ee0bd59df20abcdda08be3614e7e8...1f7746d5ddea925ffb8fa52e8abb93f78eae8002
[0.0.44.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/c25fa4e769ee0b7e47135d14b695748fe8a280cd...80cc4faa023ee0bd59df20abcdda08be3614e7e8
[0.0.43.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/00acd546022d8aec787f819fd170977fccc66366...c25fa4e769ee0b7e47135d14b695748fe8a280cd
[0.0.42.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/34071c15c56e5bf3dc11f6ebb601e42bc04f3953...00acd546022d8aec787f819fd170977fccc66366
[0.0.41.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/46b585be29f169b0e4908110f246cf9ee900206c...34071c15c56e5bf3dc11f6ebb601e42bc04f3953
[0.0.40.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/f137a717eeda5cd7b406638c499c6ccd8a740800...46b585be29f169b0e4908110f246cf9ee900206c
[0.0.39.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/d144135a2c4721021a6963b5d69748a40d55e9a4...f137a717eeda5cd7b406638c499c6ccd8a740800
[0.0.38.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/35f076b34ba80aa5773a409e57fb63488cd6bc37...d144135a2c4721021a6963b5d69748a40d55e9a4
[0.0.37.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/69f8171d6329c36ff45363ba6225231fcbe8a43e...35f076b34ba80aa5773a409e57fb63488cd6bc37
[0.0.36.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/4bf3235f1ed8aa191e9331ead85d9ae51285dcdc...69f8171d6329c36ff45363ba6225231fcbe8a43e
[0.0.35.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/20d93c2a84cb8e49d659b7ddbcdaa13933e01997...4bf3235f1ed8aa191e9331ead85d9ae51285dcdc
[0.0.34.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/c08493a9d0aede14ef4a9281beb6a2e447095ffb...20d93c2a84cb8e49d659b7ddbcdaa13933e01997
[0.0.33.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/ff9f7f35642d2349c3872d64374b7d082cba6cf0...c08493a9d0aede14ef4a9281beb6a2e447095ffb
[0.0.32.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/39cbc3045b2e24b9aa6fe17db7c9aea8d82bf941...ff9f7f35642d2349c3872d64374b7d082cba6cf0
[0.0.31.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/b612195d763a4b13d0868d630a25d9fadc215a7a...39cbc3045b2e24b9aa6fe17db7c9aea8d82bf941
[0.0.30.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/2528c616999c8c36e2eb12cf647866b342f8924a...b612195d763a4b13d0868d630a25d9fadc215a7a
[0.0.29.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/e36f09dff196281f6db00e68d492c3cb9784af5f...2528c616999c8c36e2eb12cf647866b342f8924a
[0.0.28.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/0fc7dbf844527c6f0d83889d869a6a8a05857336...e36f09dff196281f6db00e68d492c3cb9784af5f
[0.0.27.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/37c91aeccb17aba1986d84fe279b31f2a284d1fc...0fc7dbf844527c6f0d83889d869a6a8a05857336
[0.0.26.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/d5ee4e4a3e40ad89f8f287e3bc5a4c47e3ec96e3...37c91aeccb17aba1986d84fe279b31f2a284d1fc
[0.0.25.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/4f5617109d65b5f9cd764be8f8ba24d4b2afbb2a...d5ee4e4a3e40ad89f8f287e3bc5a4c47e3ec96e3
[0.0.24.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/76a21442ab722b29096894eb9eba73f1673dfc57...4f5617109d65b5f9cd764be8f8ba24d4b2afbb2a
[0.0.23.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/bb14eb59ad4afc3db1eaa0e53448c747773d2c07...76a21442ab722b29096894eb9eba73f1673dfc57
[0.0.22.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/45cd712db226f7780294ec3f782d486b7859c59f...bb14eb59ad4afc3db1eaa0e53448c747773d2c07
[0.0.21.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/aa6742d9fe02aa7686a2243ae59a5c0c91369db3...45cd712db226f7780294ec3f782d486b7859c59f
[0.0.20.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/7bff19d3394eb73276b1b34820eef60d40d0ec1d...aa6742d9fe02aa7686a2243ae59a5c0c91369db3
[0.0.19.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/9c16628e14a935ae003a9aaae059039cec6ce527...7bff19d3394eb73276b1b34820eef60d40d0ec1d
[0.0.18.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/9b0efd751ad5511a19c61dd767c571c55a119da1...9c16628e14a935ae003a9aaae059039cec6ce527
[0.0.17.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/d86ce4dffc0269734cfef173ae23be3920be00cb...9b0efd751ad5511a19c61dd767c571c55a119da1
[0.0.16.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/21fd2e3812e5ac65633959f6e48235b7c906d142...d86ce4dffc0269734cfef173ae23be3920be00cb
[0.0.15.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/acdc07745460024a82880fa0bdbf74f533ca4aae...21fd2e3812e5ac65633959f6e48235b7c906d142
[0.0.14.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/06ab17e3e72c9a1583545c37cb290f91fa08504f...acdc07745460024a82880fa0bdbf74f533ca4aae
[0.0.13.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/5a6e07f66847ed18f1dbe719abc37940d6415d85...06ab17e3e72c9a1583545c37cb290f91fa08504f
[0.0.12.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/b7d93c6abc1dd91f4bd7a8e8cf72ef0cbe835ce8...5a6e07f66847ed18f1dbe719abc37940d6415d85
[0.0.11.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/8a9cf466ec7b9882710d9ae564c8ae8018496fd6...b7d93c6abc1dd91f4bd7a8e8cf72ef0cbe835ce8
[0.0.10.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/61b133853420987665cc2e9b3da19df106a74b5d...8a9cf466ec7b9882710d9ae564c8ae8018496fd6
[0.0.9.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/1373a5ceeba801f601803b1a3743413ec17a5018...61b133853420987665cc2e9b3da19df106a74b5d
[0.0.8.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/0c92c4a2d3b3da49313c458fce328a7c7af52ec2...1373a5ceeba801f601803b1a3743413ec17a5018
[0.0.7.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/66a233e0f078a9048ec9b541dd107a71a6003b7e...0c92c4a2d3b3da49313c458fce328a7c7af52ec2
[0.0.6.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/950f93772def796a1b94620ac46618613fffcc35...66a233e0f078a9048ec9b541dd107a71a6003b7e
[0.0.5.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/e5b3784292ec3956389aab1dea5327d2d45ff947...950f93772def796a1b94620ac46618613fffcc35
[0.0.4.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/c8cded18d490b493e33698e2907897b643d8a2e6...e5b3784292ec3956389aab1dea5327d2d45ff947
[0.0.3.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/5cfb784901707edf9063233c2bc9fc4f6cb3a17d...c8cded18d490b493e33698e2907897b643d8a2e6
[0.0.2.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/068b6dd02eda17d0b917c7f032a28f215557e124...5cfb784901707edf9063233c2bc9fc4f6cb3a17d
[0.0.1.0]: https://github.com/LunchBox951/pokeemerald-rs/compare/v0.0.0.0...068b6dd02eda17d0b917c7f032a28f215557e124
[0.0.0.0]: https://github.com/LunchBox951/pokeemerald-rs/releases/tag/v0.0.0.0
