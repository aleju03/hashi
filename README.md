<p align="center">
  <img src="src-tauri/icons/128x128.png" width="96" height="96" alt="">
</p>

<h1 align="center">Hashi</h1>

<p align="center">
  Your osu!mania plays, map leaderboards and live status on <a href="https://mania-tracker.com">Mania Tracker</a>, as you play.
</p>

---

Hashi is a small desktop app for Windows and Linux. Connect it to Mania Tracker once and leave it
open while you play. Every mania map you finish on osu!stable or osu!lazer goes to your profile with
its replay, and the app shows where it ranked. While you pick a song, it shows the map's leaderboard
for the mods you have on, including boards osu! doesn't keep, like DT or HT at any rate, Invert or
No Release. Your profile can also show what you're playing right now.

It's also light. In the tray it uses next to no CPU and about 3 MB of memory on Windows.

Watched replays, other modes and plays with Auto or Relax are skipped. If Mania Tracker can't be
reached, plays wait on your computer until it's back.

## What you need

- Windows 10/11 or Linux (x86_64).
- [tosu](https://github.com/tosuapp/tosu) running while you play. If you don't have it, the app
  can download it and start it for you. On Linux, osu!lazer needs tosu's Linux build.
- To be logged in to Mania Tracker in your browser.

## Install

Download the latest version from [mania-tracker.com/hashi](https://mania-tracker.com/hashi). Every
release is also on the [Releases](../../releases) page.

On Windows, open `Hashi-Setup.exe`. It installs Hashi for your user in a few seconds, with no admin
rights, and opens it. If Windows says "Windows protected your PC", click "More info", then "Run
anyway". After that, open Hashi from the desktop or the Start menu. The app itself is
`%LOCALAPPDATA%\Hashi\Hashi.exe`, so you can delete `Hashi-Setup.exe`.

On Linux:

| Package | How to install |
|---|---|
| AppImage (any distro) | `chmod +x "Hashi_x.y.z_amd64.AppImage"`, then run it |
| .deb (Debian, Ubuntu, Mint) | `sudo apt install "./Hashi_x.y.z_amd64.deb"` |
| .rpm (Fedora) | `sudo dnf install "./Hashi-x.y.z-1.x86_64.rpm"` |
| .rpm (openSUSE) | `sudo zypper install "./Hashi-x.y.z-1.x86_64.rpm"` |

If the AppImage doesn't start and mentions `libfuse.so.2`, install FUSE 2: `sudo apt install
libfuse2` on Ubuntu 22.04 and Debian, `sudo apt install libfuse2t64` on Ubuntu 24.04 and later,
`sudo dnf install fuse-libs` on Fedora.

The tray icon needs a desktop that shows tray icons (GNOME needs the AppIndicator extension).

## Using it

Open Hashi, click "Connect to Mania Tracker" and approve the connection in your browser. Then leave
it open and play. Plays you finish while it isn't connected aren't sent later.

Each finished map appears at the top of the list and gets a stamp once Mania Tracker has checked it,
along with where it placed on the map's leaderboard. Click a play to open it on Mania Tracker, or
its rank to see the map's leaderboard, which you can narrow to a country or your friends, or search
for a player. While you're picking a song, the podium button next to the gear opens the selected
map's leaderboard. A play that hasn't been sent yet can be held back or removed.

The chart icon next to a play's time opens its analysis, worked out on your computer from the
replay: how early or late you hit and let go of long notes, each finger's and each hand's accuracy,
how it went over the course of the map, and what stands out, with what fixing it would have been
worth on that play. On 5K, 7K and 9K the middle column counts for the hand you pick in the settings.
Plays you finish while sending is off are still analyzed, they just aren't sent. If you'd rather not
have any of it, turn off Analysis in the settings. Goals and your profile go with it.

Closing the window quits the app. The hide to tray button keeps it running in the background using
very little memory.

The gear opens the settings, where you can choose whether to send your plays, share your live
status on your profile, or both. You can also have Hashi open tosu, open on startup or start in
the tray, and change the theme. Disconnect signs the app out and revokes its access.

New versions show a note at the top of the app and install from there. Updates are signed, and the
app only installs files signed with Hashi's release key.

## What gets sent

For each play: the replay osu! saved, the chart file if Mania Tracker doesn't have that version,
the app's version and whether it was stable or lazer. With the live status on, also what you're
doing in osu! and the current map, mods and speed. Apart from Mania Tracker, the app only talks to
tosu on your computer, osu!'s servers for your avatar, and GitHub for updates and tosu downloads.

## Troubleshooting

If the app says Linux blocks tosu from reading the game, run the command it shows once (it looks
like `sudo setcap cap_sys_ptrace=eip /path/to/tosu`) and restart tosu. You'll need it again after
updating tosu.

If a play doesn't show up, check that the line under your name doesn't say tosu isn't running. Only
finished plays count. If a play says "Not submitted", the reason is written under the stamp.

If you route apps through a VPN or a firewall, add `%LOCALAPPDATA%\Hashi\Hashi.exe`, not the setup
file.

For anything else, the log `bridge.log` in the data folder is the place to look, and worth
attaching to a report.

## Your data

Settings, unsent plays, the log and the connection live in `%APPDATA%\Hashi` on Windows and
`~/.config/hashi` on Linux. Deleting that folder signs the app out and forgets unsent plays.
Uninstall it the usual way; on Windows the uninstaller asks whether to delete your data too.

## License

MIT, see [LICENSE](LICENSE). Tracker Round in `counter/fonts`, built on Outfit, is under the SIL Open Font License.
