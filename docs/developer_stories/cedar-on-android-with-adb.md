# Installing Cedar on Android with ADB

This guide installs the Cedar app on an Android phone by sideloading its APK from a computer over USB with Android Debug Bridge (`adb`). The whole process takes about five minutes.

[Cedar](https://getcedar.io) for Android is not on Google Play yet; it is planned for the coming months. Until then, `adb` is the way to install the current APK.

The app is a client for a Cedar you already run on a Mac, a Windows PC or a Linux machine. If you don't have one yet, start with the [install guide](https://getcedar.io/install.html).

```text
Computer with adb -> USB cable -> Android phone -> Cedar app
```

## What you need

- An Android phone (Android 8 or newer) and a USB cable that carries data, not a charge-only one
- A Mac, Windows or Linux computer
- The Cedar APK, `Cedar-<version>-android.apk`, from the latest [GitHub Release](https://github.com/hanneshapke/cedar/releases/latest)

The commands below use `Cedar-0.7.0-android.apk` as the example. Use the name of the file you downloaded.

## 1. Get the Android platform tools

SDK Platform-Tools contains `adb`. Download it from the official Android page: [developer.android.com/tools/releases/platform-tools](https://developer.android.com/tools/releases/platform-tools).

1. Open the page and download the Platform-Tools package for your operating system.
2. Unzip it. You get a folder named `platform-tools` that contains the `adb` program.
3. Open a terminal in that folder:

   ```bash
   cd ~/Downloads/platform-tools
   ```

4. Put the Cedar APK somewhere easy to reach, such as an `apps` folder inside `platform-tools`.
5. Check that `adb` works:

   ```bash
   ./adb version
   ```

On Windows, use `adb.exe` or `.\adb` instead of `./adb`.

## 2. Enable Developer options and USB debugging

Android hides these settings until you unlock them. Menu names vary slightly by manufacturer.

1. Open **Settings > About phone** (on some phones **Settings > System > About phone**).
2. Tap **Build number** seven times. Enter your screen lock if asked. A message says "You are now a developer!"
3. Go back to **Settings > System > Developer options** (on some phones it sits directly in Settings).
4. Turn the **Developer options** switch on.
5. Scroll down and turn on **USB debugging**. Confirm the warning.
6. Optional: on Xiaomi, Oppo and some other brands, also turn on **Install via USB**.

## 3. Allow the computer on the phone

Plug the phone into the computer and unlock it. Android shows an **Allow USB debugging?** pop-up with the computer's RSA key fingerprint. Approving it is what lets the computer install apps on the phone.

1. Tick **Always allow from this computer** so you are not asked again.
2. Tap **Allow**.
3. If the phone offers a USB mode, choose **File transfer** rather than charging only.
4. Confirm the phone is accepted:

   ```bash
   ./adb devices
   ```

The phone must be listed as `device`. If it is listed as `unauthorized`, the pop-up has not been approved yet.

Some phones show a second prompt when you install, such as **Install via USB**. Tap **Install** or **Allow** there too.

## 4. Install Cedar

From the `platform-tools` folder, run:

```bash
./adb install apps/Cedar-0.7.0-android.apk
```

`adb` prints `Performing Streamed Install` and then `Success`. Cedar now appears in the phone's app launcher.

To update an existing install with a newer APK, add `-r`:

```bash
./adb install -r apps/Cedar-0.7.0-android.apk
```

## 5. Pair the app with your Cedar

The app reaches your Cedar over HTTPS, most simply with Tailscale.

1. Install Tailscale from Google Play on the phone and log in to the same account as the computer that runs Cedar.
2. Open Cedar on the phone, tap **Scan QR Code** in Settings, and scan the code your Cedar shows: **Phone Access…** in the Mac or Windows app, or **Settings → Phone** in the chat on Linux.

The [install guide](https://getcedar.io/install.html?mobile=yes&phone=android) has the steps for your computer, and the [Android README](../../android/README.md) covers notifications and everything else the app does.

Once the app is installed you can turn **USB debugging** off again. Turn it back on when you update.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| `./adb devices` lists nothing | Try another cable or port, set the USB mode to File transfer, and confirm USB debugging is on. |
| Device shows `unauthorized` | Unlock the phone and tap **Allow** on the USB debugging pop-up. If it never appears, use **Revoke USB debugging authorizations** in Developer options, then replug. |
| `INSTALL_FAILED_USER_RESTRICTED` | Turn on **Install via USB** (or **USB debugging (Security settings)**) in Developer options and accept the install prompt. |
| `INSTALL_FAILED_UPDATE_INCOMPATIBLE` | An older Cedar is signed differently. Uninstall it, then install again. |
| `adb: command not found` | Run it from the `platform-tools` folder with `./adb`, or add that folder to your PATH. |

## Sources

- [Cedar](https://getcedar.io)
- [Cedar for Android README](https://github.com/hanneshapke/cedar/blob/main/android/README.md)
- [Cedar releases](https://github.com/hanneshapke/cedar/releases)
- [SDK Platform-Tools](https://developer.android.com/tools/releases/platform-tools)
- [Android Debug Bridge (adb)](https://developer.android.com/tools/adb)
