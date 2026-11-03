# Installing Skate 3 Remaster

**Skate 3 Remaster is an unofficial fan project, not affiliated with or endorsed by EA. It includes no game files.
The launcher builds the game on your PC from your own copies of the games.**

## What you need

- Windows 10 or 11, 64-bit, with a graphics card that has at least 4 GB of video memory.
- About **50 GB of free disk space** for the first build (the finished game takes about 14 GB). If your game files
  are on another drive than the game, some packages are copied instead of linked, so keep more space free.
- An **internet connection** for the first build. It downloads the build tools (Java, unluac, vgmstream and
  ffmpeg) and one public source file from Xenia's GitHub repository, which holds the disc decryption key. No game
  files are downloaded.
- Windows **long file paths** turned on (see below).
- An XInput controller, for example an Xbox controller.
- Your own copies of **all 18 game files**: the Skate 3 disc, Skate 3 Title Update 3, the nine Skate 3 DLC packs,
  the Skate 2 disc (European), the three Skate 2 DLC packs, the Skate disc (European) and two community mods
  (RPCS3 New San Van and Skate1Map). The
  [game files guide](https://github.com/daytryan/S3R/blob/main/docs/source-files-guide.md) explains
  each one.

## Which files work

- **Discs must be `.iso` images** (plain or XGD). An extracted folder, a Games on Demand copy, a split ISO or a
  `default.xex` on its own is not accepted.
- **Skate 1 and Skate 2 must be the European discs.** US discs show as "Not usable".
- DLC packs and the title update are Xbox 360 content packages, copied from the console unchanged.
- Mods are folders. Keep them exactly as they were released.
- File names and folders don't matter: the launcher recognises each file by what is inside it. Subfolders are fine.

## Install

1. Unzip `Skate3Remaster-launcher.zip` into a folder of your own, for example `C:\Games\Skate 3 Remaster`. Not
   inside Program Files: the launcher can't update itself there. Open **Skate 3 Remaster.exe**.
   - If Windows shows "Windows protected your PC", click **More info**, then **Run anyway**.
   - If your antivirus flags `skate3setup.exe`, allow it. It is the build helper, a packaged Python program, and
     the build can't run without it.
2. Press **Install** to download the game program (about 100 MB, no game content).
3. Open **Files**, press **Choose folder** and pick the folder with your game files. The launcher checks every item
   and shows Found, Missing or Not usable for each one.
4. When everything is found, press **Build**. The first build takes a while (about 75 minutes on a fast PC with an
   SSD). You can stop it; it continues where it left off.
5. Press **Play**.

## Long file paths

The build writes some file paths longer than Windows' old 260-character limit, so long paths must be on before you
build. If they are off, the launcher says so and shows a **Turn it on** button. Windows asks for an admin's
permission once. On a standard user account, ask someone with an admin account to turn it on.

## A file shows Missing or Not usable

- **OneDrive or Dropbox:** files that are only in the cloud are skipped, so they show as Missing. Right-click the
  folder and choose **Always keep on this device**, wait for the download, then check again.
- **Wrong version:** a different Title Update (not TU3) or another release of a DLC pack shows as Missing. A disc
  from another release, such as a US Skate 1 or Skate 2 disc, shows as Not usable.
- **Wrong format:** an extracted disc folder or a Games on Demand copy shows as Missing. Make an `.iso` image of the
  disc instead.
- **Cut short:** a package that was not copied in full shows as Not usable ("incomplete"). Copy it again.
- **Files differ:** when the build starts, every file is checked in full. A message like "N files differ" means a
  disc image is damaged or a mod is not the release the build expects. Make the ISO again, or get the mod release
  the game files guide describes.

## Playing online

Online play goes through Steam: keep Steam open and signed in. The game only connects to Steam once you sign in
to **Online** in the game, so sign in there **before** you accept a Steam invite or use **Join Game** in Steam's
friends list. Otherwise Steam starts Spacewar instead of joining. Everyone in a session needs the same build.

## Updates

The launcher checks for a new release each time it opens. Press **Update**, then **Build** if the update needs part
of the game rebuilt. Your mods, settings and saves are kept.

## Uninstall

Open **Settings → Storage** in the launcher. **Uninstall game** deletes the built game and keeps the launcher.
**Uninstall everything** also removes the launcher, its settings, logs, crash reports and desktop shortcut. Both keep
your saves in the game folder's `data\settings`, your imported characters and your own music and mods, and never
touch your game files.

## Help

- Crash reports are saved in `%LOCALAPPDATA%\Skate3RustEngine\CrashReports`. A report is sent only if you press
  **Send report** ([what a report holds](crash-reports.md)).
- Ask on the [Discord](https://discord.gg/PC2W9K4xan) or open a
  [GitHub issue](https://github.com/daytryan/S3R/issues). Paste the launcher's **Copy report** text.
  Never attach ISOs or other game files.
