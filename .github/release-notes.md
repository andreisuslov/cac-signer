Sign PDFs with the certificate on your CAC, on a Mac, without Adobe Acrobat.

## Download

- **CAC-Signer.dmg** (below, under Assets): open it and drag **CAC Signer** into **Applications**.
- **CAC-Signer.zip**: the same app in a ZIP file. Double-click it, then drag **CAC Signer** into **Applications**.

CAC Signer {version} runs on Macs with Apple silicon or Intel, with macOS 12 or later.

## Opening it the first time

CAC Signer is not signed or notarized by Apple ([what that means](https://support.apple.com/en-us/102445)). macOS stops apps like that the first time you open them, so you approve it once:

1. Open **CAC Signer** from Applications. macOS says it could not verify the app. Click **Done**.
2. Open **System Settings**, click **Privacy & Security**, and scroll down to **Security**. Next to "CAC Signer was blocked to protect your Mac", click **Open Anyway**. The button is there for about an hour after step 1.
3. Confirm with your Mac's password or Touch ID, and click **Open Anyway** if macOS asks once more.

From then on, CAC Signer opens like any other app. On macOS 12, the button is in System Preferences, under Security & Privacy, on the General tab. Apple's guide to these steps: [Open a Mac app from an unknown developer](https://support.apple.com/guide/mac-help/open-a-mac-app-from-an-unknown-developer-mh40616/mac).

## Before you sign

Insert your CAC in a card reader your Mac recognizes. If Keychain Access shows your certificates when the card is in, CAC Signer can use them. Short video instructions are on the [project page](https://github.com/andreisuslov/cac-signer#readme).

## Checksums (SHA-256)
