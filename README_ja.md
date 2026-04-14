# y2skk

> **⚠ 開発中バージョン**
>
> y2skk は現在開発中です。バージョン間で破壊的変更が発生する場合があり、
> 一部の機能は未実装または不完全な場合があります。
> 本番環境での使用は自己責任でお願いします。

Linux 向けの SKK 日本語入力メソッドです。Rust で実装しています。

y2skk はデーモン (`y2skk-daemon`) として動作し、D-Bus 経由で機能を提供します。
GTK3 / Qt6 / XIM それぞれのアダプターがデーモンに接続する設計なので、
辞書やセッション状態はすべてのアプリケーションで共有されます。

---

## 対応環境

| コンポーネント | 状態 |
|---------------|------|
| GTK3 アプリケーション | ✅ 動作確認済み |
| Qt6 アプリケーション | ✅ 動作確認済み |
| XIM クライアント（xterm 等） | ✅ 動作確認済み（`y2skk-xim` 経由） |
| KDE Plasma（X11） | ✅ 主要ターゲット |
| Wayland / GTK4 | 🚧 未対応 |

---

## 機能

- **SKK プロトコル** — ひらがな / カタカナ / 半角カタカナ / 全角英数 / 半角英数モード
- **かな入力テーブル** — ローマ字、AZIK（US / JP）、DvorakJP（US / JP）
- **辞書対応** — UTF-8 / EUC-JP / EUC-JISX0213 辞書、複数辞書・優先度指定
- **ユーザー辞書** — 単語登録（`▼` モード）、確定時に自動保存
- **候補選択** — インライン表示（件数設定可）後、リストモードに移行
- **タブ補完** — `▽` モードでのゴーストテキスト補完（uim-skk 準拠）
- **IME トグル** — Shift+Space でひらがな ↔ 半角英数を切り替え（設定変更可）
- **モードインジケーター** — モード変更時にフローティングポップアップ表示（自動消去・タイムアウト設定可）
- **コード入力** — `\XXXX`（JIS コード）、`\uXXXX`（Unicode コードポイント）
- **Abbrev モード** — ローマ字で直接辞書検索（`/` キー）
- **vi 互換 Esc** — オプション機能。通常入力フェーズで Esc を押すと ASCII モードに切り替わる（設定変更可）
- **XIM サーバー** — D-Bus 経由でデーモンに接続する独立バイナリ `y2skk-xim`
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
| Qt6 + プライベートヘッダ | Qt6 プラグインのビルド |
| pkg-config | ビルドシステムが使用 |

各パッケージはディストリビューションのパッケージマネージャーでインストールしてください。
GTK3 / Qt6 パッケージはそれぞれのアダプターをビルドする場合のみ必要です。

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

全コンポーネントをビルドして `~/.local/` 以下にインストールします。
詳細やオプション・システム全体へのインストール方法は [INSTALL.md](INSTALL.md) を参照してください。

### 2. 環境変数の設定

**KDE Plasma** の場合、`~/.config/plasma-workspace/env/y2skk.sh` を作成します：

```sh
export XMODIFIERS=@im=y2skk      # XIM クライアント（xterm、Chromium 等）
export GTK_IM_MODULE=y2skk       # GTK3 アプリケーション
export QT_IM_MODULE=y2skk        # Qt6 アプリケーション
# デフォルト（~/.local/）インストール時はさらに以下も必要:
export GTK_IM_MODULE_FILE="$HOME/.config/gtk-3.0/gtk.immodules"
export QT_PLUGIN_PATH="$HOME/.local/lib/qt6/plugins:$QT_PLUGIN_PATH"
```

> `cargo xtask install --system` でシステムインストールした場合、最後の 2 行は不要です。

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

---

## 設定

設定ファイルは `~/.config/y2skk/config.toml` に置きます。
全オプションの説明は [`dist/config.toml.example`](dist/config.toml.example) を参照してください。

---

## ライセンス

MIT
