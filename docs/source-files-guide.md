# Source files guide

Skate 3 Remaster ships no game data. The launcher builds the game from your own copies of the Skate games, their
downloadable content and three community mods. This page explains, item by item, what each one is and where it comes
from. The launcher's Source files page links to the matching section here ("How do I get this?").

Use only copies you own: discs you bought and content your Xbox Live account bought or downloaded. This project
doesn't provide download links for game files.

## How the launcher finds your files

- Put everything in one folder, for example `Skate Source Files`. Subfolders are fine. The suggested layout numbers
  them `01` to `09`, as in each section below, but any layout works: the launcher identifies each file by what's inside
  it, not by its name or folder.
- Discs must be ISO images (`.iso`, plain or XGD). An extracted disc folder, a Games on Demand copy, a split ISO or a
  `default.xex` on its own is not accepted and shows as Missing. Downloadable content and title updates are Xbox 360
  content packages (no file extension, a name like `5841122D...`). Mods are folders.
- Files that OneDrive or Dropbox keep only online show as Missing. Right-click the folder and choose **Always keep on
  this device** first.
- Before the first build the launcher checks every file in full. A file that doesn't match what the build expects
  shows its reason on the Source files page: "a different region or revision" for a disc from another release, or
  "incomplete (X of Y bytes)" for a package that was cut short while copying.

## Getting files off an Xbox 360

Discs: a PC drive can't read an Xbox 360 game disc on its own. To make an ISO of your disc you need a PC drive that
can read Xbox 360 discs, or a console set up to copy a disc. Whatever you use, the result must be a single `.iso`
image of the whole disc. A disc installed to the console's hard drive, an extracted folder or a Games on Demand copy
is not accepted, and neither is an ISO split into parts.

Downloadable content and title updates: these live on the console's hard drive or memory unit under
`Content\0000000000000000\<title ID>\`, in a `00000002` folder (downloadable content) or a `000B0000` folder (title
updates). Skate 3's title ID is `454108E6` and Skate 2's is `4541087F`. Copy the package files from there to your PC
unchanged, for example with a USB drive formatted on the console and a tool that reads the Xbox 360 USB format.

<a id="s3-disc"></a>
## Skate 3 disc (01)

The Skate 3 game disc, USA/Europe release, as one ISO image. It is the base of the whole build: the city, the skaters,
the menus and the sounds. The launcher identifies it by the disc's `default.xex`. A disc from another region or
revision shows "a different region or revision".

<a id="s3-tu3"></a>
## Skate 3 Title Update 3 (02)

The last Skate 3 title update, the package the console downloads when you first play online. Look in
`Content\0000000000000000\454108E6\000B0000\`. If the folder is empty, start Skate 3 on the console while
it is online so it downloads the update, then copy it. An older title update (TU1 or TU2) is not recognised and shows
as Missing.

<a id="s3-dlc-sanvan"></a>
## San Van Party Pack (03)

Skate 3 downloadable content (`Content\0000000000000000\454108E6\00000002\`). There are two copies of this pack in
circulation; either works, and when both are present the launcher picks one.

<a id="s3-dlc-hawaii"></a>
## Danny Way's Hawaiian Dream (03)

Skate 3 downloadable content. Found under
`Content\0000000000000000\454108E6\00000002\`.

<a id="s3-dlc-maloof"></a>
## Maloof Money Cup 2010 NYC (03)

Skate 3 downloadable content. Found under
`Content\0000000000000000\454108E6\00000002\`.

<a id="s3-dlc-afterdark"></a>
## After Dark (03)

Skate 3 downloadable content. Found under `Content\0000000000000000\454108E6\00000002\`.

<a id="s3-dlc-create"></a>
## Skate.Create Upgrade Pack (03)

Skate 3 downloadable content. Found under
`Content\0000000000000000\454108E6\00000002\`.

<a id="s3-dlc-blackbox"></a>
## Black Box Distribution Skate Park (03)

Skate 3 downloadable content. A small package (about 116 KB). Found under
`Content\0000000000000000\454108E6\00000002\`.

<a id="s3-dlc-filmer"></a>
## Filmer Pack (03)

Skate 3 downloadable content. A small unlock package (about 116 KB). Found under
`Content\0000000000000000\454108E6\00000002\`.

<a id="s3-dlc-share"></a>
## Skate Share Pack (03)

Skate 3 downloadable content. A small unlock package (about 116 KB). Found under
`Content\0000000000000000\454108E6\00000002\`.

<a id="s3-dlc-money"></a>
## Time is Money (03)

Skate 3 downloadable content. A small unlock package (about 120 KB). Found under
`Content\0000000000000000\454108E6\00000002\`.

<a id="s2-disc"></a>
## Skate 2 disc (04)

The Skate 2 game disc, European release, as one ISO image. The Skate 2 career, tutorial, clothing and online modes
come from it (its maps come from the New San Van mod below). A disc from another region, such as the US disc, shows
as Not usable: "a different region or revision".

<a id="s2-dlc-maloof"></a>
## Skate 2 Maloof Money Cup Pack (05)

Skate 2 downloadable content (`Content\0000000000000000\4541087F\00000002\`).

<a id="s2-dlc-filmer"></a>
## Skate 2 Filmer Pack (05)

Skate 2 downloadable content (`Content\0000000000000000\4541087F\00000002\`).

<a id="s2-dlc-throwback"></a>
## Throwback Pack (05)

Skate 2 downloadable content (`Content\0000000000000000\4541087F\00000002\`). A small unlock package (about 60 KB).

<a id="s1-disc"></a>
## Skate disc (06)

The first Skate game ("Skate."), European release, as one ISO image. The Skate 1 challenges, clothing and HUD come from
it. A disc from another region, such as the US disc, shows as Not usable: "a different region or revision".

<a id="mod-newsanvan"></a>
## RPCS3 New San Van mod (07)

A community mod for the PlayStation 3 version of Skate 3, as its release folder. It contains the New San Vanelona and
Skate 2 park streams the build converts into maps. The launcher finds it by
`PS3_GAME/USRDIR/data/content/world/stream/DIST_Factory_Proxy/cPres_Global.psf` (about 55 MB; the whole mod is about
2 GB). Keep the folder as it was released: the build checks every file in it. The release the build expects has
2,510 files, 2,198,703,124 bytes in all: 2,504 under `PS3_GAME/USRDIR` and 6 under `dev_hdd0/game`.

<a id="mod-skate1map"></a>
## Skate1Map mod (08)

A community mod that brings Skate 1's San Vanelona to the PlayStation 3 version of Skate 3. The launcher finds it by
`CUSTOMM000000000/custom_00000000.big.edat` (about 3.2 GB). Keep the folder as it was released. The release the
build expects has two files: that one (3,457,139,328 bytes) and `README.txt` (248 bytes), which must be there
unchanged too.
