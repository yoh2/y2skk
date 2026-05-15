# y2skk

> **⚠ 開発中バージョン**
>
> y2skk は現在開発中です。バージョン間で破壊的変更が発生する場合があり、
> 一部の機能は未実装または不完全な場合があります。
> 本番環境での使用は自己責任でお願いします。

Linux 向けの SKK 日本語入力メソッドです。Rust で実装しています。

y2skk はデーモン (`y2skk-daemon`) として動作し、D-Bus 経由で機能を提供します。
GTK3 / GTK4 / Qt6 / XIM / Wayland それぞれのアダプターがデーモンに接続する
設計なので、辞書やセッション状態はすべてのアプリケーションで共有されます。

---

## 対応環境

| コンポーネント | 状態 |
|---------------|------|
| GTK3 アプリケーション | ✅ 動作確認済み |
| GTK4 アプリケーション | ✅ 動作確認済み |
| Qt6 アプリケーション | ✅ 動作確認済み |
| XIM クライアント（xterm 等） | ✅ 動作確認済み（`y2skk-xim` 経由） |
| KDE Plasma（X11） | ✅ 主要ターゲット |
| KDE Plasma（Wayland） | 🧪 **実験的** — 下記注意参照 |

### Wayland サポートについて（実験的・改善中）

Wayland アダプター（`y2skk-wayland`）は KDE 独自の `zwp_input_method_v1` 拡張を
ベースにしているため、現時点では KWin（KDE Plasma 5/6）でしか動作せず、他の
Wayland コンポジタへの移植性はありません。`zwp_input_method_v2` への対応は
**予定なし**（KWin が advertise しておらず、手元でテストできる v2 対応コンポジタ
も無いため。GTK4/Wayland プロジェクトメモ参照）。

直近の修正で過去に報告されていた主要な不具合（Slack / Electron での Enter /
Backspace の半分失敗、画面下端での候補ウィンドウ残留、長時間使用時の素通し化）
は概ね解消しましたが、依然として実験的位置付けです。パッケージング、KDE 以外
のコンポジタ対応、幅広いアプリでの検証は未整備です。

KDE Plasma Wayland セッションでは日常用途で使える状態になっています。KDE 以外
の Wayland では引き続き X11 経路（XIM + GTK3 / GTK4 / Qt6 アダプター）を
使用してください。

---

## 機能

- **SKK プロトコル** — ひらがな / カタカナ / 半角カタカナ / 全角英数 / 半角英数モード
- **かな入力テーブル** — ローマ字、AZIK（US / JP）、DvorakJP（US / JP）
- **辞書対応** — UTF-8 / EUC-JP / EUC-JISX0213 辞書、複数辞書・優先度指定
- **ユーザー辞書** — 単語登録（`▼` モード）、確定時に自動保存
- **数値変換** — DDSKK 標準の `#0`–`#3` / `#5` / `#9` に加え、y2skk 独自拡張の
  `#6` / `#7` / `#a` / `#b` / `#c` に対応。辞書にエントリーが無いテンプレート
  見出しでも合成候補を生成
- **候補選択** — インライン表示（件数設定可）後、リストモードに移行
- **タブ補完** — `▽` モードでのゴーストテキスト補完（uim-skk 準拠）
- **IME トグル** — Shift+Space でひらがな ↔ 半角英数を切り替え（設定変更可）
- **モードインジケーター** — モード変更時にフローティングポップアップ表示（自動消去・タイムアウト設定可）
- **コード入力** — `\XXXX`（JIS コード）、`\uXXXX`（Unicode コードポイント）
- **Abbrev モード** — ローマ字で直接辞書検索（`/` キー）
- **vi 互換 Esc** — オプション機能。通常入力フェーズで Esc を押すと ASCII モードに切り替わる（設定変更可）
- **XIM サーバー** — D-Bus 経由でデーモンに接続する独立バイナリ `y2skk-xim`
- **GTK3 / GTK4 IM モジュール** — `adapter-gtk3`（GTK3 の従来 IM module ABI）と
  `adapter-gtk4`（GIO `gtk-im-module` extension point）の両方が
  `GTK_IM_MODULE=y2skk` で読み込まれる
- **Wayland アダプター（実験的）** — `zwp_input_method_v1` ベースの独立バイナリ
  `y2skk-wayland`（KDE 限定）。上の警告を参照
- **デーモン再接続・フェイルオープン** — デーモン再起動中もアダプターは動作を継続。
  D-Bus エラー発生時は UI をブロックせず passthrough にフォールバック
- **設定検証** — `y2skk-daemon --check-config [--config <PATH>]` で設定ファイルの妥当性を検証してデーモンを起動せずに終了する

---

## 必要なもの

### 実行時

- D-Bus セッションバス

### ビルド時

| 依存パッケージ | 用途 |
|--------------|------|
| Rust + Cargo | 全コンポーネントのビルド |
| cmake ≥ 3.21 | Qt6 プラグインのビルド |
| GTK3 開発ヘッダ | GTK3 IM モジュールのビルド |
| GTK4 開発ヘッダ | GTK4 IM モジュールのビルド |
| Qt6 + プライベートヘッダ | Qt6 プラグインのビルド |
| pkg-config | ビルドシステムが使用 |
| `gio-querymodules`（`glib2` に同梱） | GTK4 IM モジュールインストール後の GIO キャッシュ更新（インストール時のみ。`cargo build` 自体では使用しない） |

各パッケージはディストリビューションのパッケージマネージャーでインストールしてください。
GTK3 / GTK4 / Qt6 パッケージはそれぞれのアダプターをビルドする場合のみ必要です。

### 辞書

y2skk を使うには SKK 辞書が最低 1 つ必要です。
[skk-dev/dict](https://github.com/skk-dev/dict) から入手するか、ディストリビューションのパッケージマネージャーでインストールしてください。

---

## クイックスタート

:warning: このインストール方法はうまく動作しない可能性が高いです。

### 1. ビルド & インストール

```sh
cargo xtask install
```

このコマンドは daemon / XIM サーバー / GTK3 / GTK4 / Qt6 をビルドして
`~/.local/` 以下にインストールします。実験的な Wayland アダプターは
**デフォルトではインストールされません**。明示的に有効化するには `--wayland`
を指定します：

```sh
cargo xtask install --wayland         # Wayland アダプターのみ
cargo xtask install --daemon --xim --gtk3 --gtk4 --qt6 --wayland   # 全部入り
```

詳細やオプション・システム全体へのインストール方法は [INSTALL.md](INSTALL.md) を参照してください。

### 2. 環境変数の設定

**KDE Plasma** の場合、`~/.config/plasma-workspace/env/y2skk.sh` を作成します：

```sh
export XMODIFIERS=@im=y2skk      # XIM クライアント（xterm、Chromium 等）
export GTK_IM_MODULE=y2skk       # GTK3 / GTK4 アプリケーション
export QT_IM_MODULE=y2skk        # Qt6 アプリケーション
# デフォルト（~/.local/）インストール時はさらに以下も必要:
export GTK_IM_MODULE_FILE="$HOME/.config/gtk-3.0/gtk.immodules"
export GIO_EXTRA_MODULES="$HOME/.local/lib/gtk-4.0/immodules:$GIO_EXTRA_MODULES"
export QT_PLUGIN_PATH="$HOME/.local/lib/qt6/plugins:$QT_PLUGIN_PATH"
```

> `cargo xtask install --system` でシステムインストールした場合、最後の 3 行は不要です。
>
> `GIO_EXTRA_MODULES` は GTK4 アダプター用です。GIO はデフォルトでは
> `~/.local/lib/gtk-4.0/immodules/` を scan しないため、この環境変数で path
> を追加する必要があります。設定し忘れると GTK4 アプリで
> 「No IM module matching y2skk found」warning が出ます。

ログアウト・ログインし直すか、ファイルを `source` して反映させてください。

### 3. 辞書の設定

設定ファイルのサンプルをコピーして辞書パスを編集します：

```sh
mkdir -p ~/.config/y2skk
cp dist/config.toml.example ~/.config/y2skk/config.toml
$EDITOR ~/.config/y2skk/config.toml
```

### 4. サービスの起動

```sh
systemctl --user enable --now y2skk-daemon
systemctl --user enable --now y2skk-xim
```

ログの確認：

```sh
journalctl --user -u y2skk-daemon -f
journalctl --user -u y2skk-xim -f
```

> Wayland アダプター（`y2skk-wayland`）は systemd サービスではなく、KWin の
> Virtual Keyboard 機構から起動されます。`cargo xtask install --wayland` でインストール
> したあと、*システム設定 → 入力デバイス → 仮想キーボード → y2skk* を選択して
> 有効化してください。

---

## 設定

設定ファイルは `~/.config/y2skk/config.toml` に置きます。
全オプションの説明は [`dist/config.toml.example`](dist/config.toml.example) を参照してください。

---

## ライセンス

MIT
