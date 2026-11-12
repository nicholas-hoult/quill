#!/bin/zsh
# 打包成 macOS 应用：release 构建 → Quill.app → ad-hoc 签名 → zip / dmg（输出到 dist/）
# 用法：scripts/bundle.sh            （已有 release 产物则直接打包）
#       BUILD=1 scripts/bundle.sh    （先构建）
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
TARGET=${CARGO_TARGET_DIR:-target-release}
ARCH=$(uname -m)
[[ "${BUILD:-0}" == "1" ]] && CARGO_TARGET_DIR=$TARGET cargo build --release
BIN=${BIN:-$TARGET/release/quill}   # 也可 BIN=target/debug/quill 先打一个开发构建包
[[ -x "$BIN" ]] || { echo "找不到 $BIN，先 BUILD=1 构建"; exit 1; }
SUFFIX=${SUFFIX:-}

APP=dist/Quill.app
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/quill"
strip -S "$APP/Contents/MacOS/quill" 2>/dev/null || true   # 去掉调试符号，体积小很多

# 图标：PNG → iconset → icns
ICONSET=$(mktemp -d)/Quill.iconset && mkdir -p "$ICONSET"
python3 scripts/make_icon.py "$ICONSET/base.png"
for s in 16 32 128 256 512; do
  sips -z $s $s "$ICONSET/base.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) "$ICONSET/base.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
rm "$ICONSET/base.png"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Quill</string>
  <key>CFBundleDisplayName</key><string>Quill</string>
  <key>CFBundleIdentifier</key><string>com.usight.quill</string>
  <key>CFBundleExecutable</key><string>quill</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict></plist>
PLIST

# 本机 ad-hoc 签名（没有开发者证书；换机器首次打开需「右键 → 打开」）
codesign --force --deep -s - "$APP"
codesign --verify --deep --strict "$APP" && echo "签名校验通过（ad-hoc）"

rm -f "dist/Quill-${VERSION}${SUFFIX}-${ARCH}.zip" "dist/Quill-${VERSION}${SUFFIX}-${ARCH}.dmg"
ditto -c -k --keepParent "$APP" "dist/Quill-${VERSION}${SUFFIX}-${ARCH}.zip"
hdiutil create -quiet -volname Quill -srcfolder "$APP" -ov -format UDZO "dist/Quill-${VERSION}${SUFFIX}-${ARCH}.dmg"
ls -la dist
