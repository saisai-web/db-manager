#!/usr/bin/env bash
# Mac 用の SQLTool.app を作り、署名して、dmg にする。公証の資格情報があれば公証してステープルする。
#
#   scripts/package-mac.sh [arm64|x86_64]      (省略時はこの Mac の種類)
#
# 環境変数:
#   SIGN_IDENTITY   署名に使う ID。省略時はキーチェーンの "Developer ID Application" を探す。"-" なら ad-hoc 署名(配布不可)
#   NOTARY_PROFILE  `xcrun notarytool store-credentials` で保存したプロファイル名(既定 sqltool)。無ければ公証を飛ばす
set -euo pipefail

cd "$(dirname "$0")/.."
ARCH="${1:-$(uname -m)}"
case "$ARCH" in
  arm64 | aarch64) ARCH=arm64; TARGET=aarch64-apple-darwin ;;
  x86_64) TARGET=x86_64-apple-darwin ;;
  *) echo "未対応の種類: $ARCH" >&2; exit 2 ;;
esac

# ワークスペース共通のバージョン(Cargo.toml の [workspace.package])
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT="dist/mac-$ARCH"
APP="$OUT/SQLTool.app"
DMG="dist/SQLTool-$VERSION-mac-$ARCH.dmg"
IDENTITY="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)}"
NOTARY_PROFILE="${NOTARY_PROFILE:-sqltool}"

cargo build --release -p app --target "$TARGET"

rm -rf "$OUT" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$TARGET/release/sqltool" "$APP/Contents/MacOS/sqltool"
cp crates/app/assets/icon.icns "$APP/Contents/Resources/icon.icns"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"

if [ -z "$IDENTITY" ]; then
  echo "署名 ID が見つかりません。SIGN_IDENTITY を指定するか、Developer ID Application 証明書を入れてください(ad-hoc にするなら SIGN_IDENTITY=-)" >&2
  exit 1
fi
# Hardened Runtime(公証の必須条件)と、セキュアタイムスタンプを付けて署名する
if [ "$IDENTITY" = "-" ]; then
  codesign --force --sign - "$APP"
else
  codesign --force --timestamp --options runtime --sign "$IDENTITY" "$APP"
fi
codesign --verify --strict --verbose=2 "$APP"

# dmg: アプリと /Applications へのリンクだけを入れた圧縮イメージ
STAGE="$OUT/dmg"
mkdir -p "$STAGE" dist
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "SQLTool" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
[ "$IDENTITY" = "-" ] || codesign --force --timestamp --sign "$IDENTITY" "$DMG"

if [ "$IDENTITY" != "-" ] && xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature -v "$DMG"
else
  echo "注意: 公証していません(プロファイル '$NOTARY_PROFILE' が無いか、ad-hoc 署名)。他の Mac で開くと警告されます。" >&2
fi
echo "できました: $DMG"
