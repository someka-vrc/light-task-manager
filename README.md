# Light Task Manager

Windows 11 向けの軽量なタスクマネージャーです。Rust と Win32 で書かれた単一の EXE で、
画面はすべて CPU で描画します。要件は [docs/requirements.md](docs/requirements.md) にあります。

## ビルドと実行

管理者権限は不要です。`rust-toolchain.toml` が GNU ツールチェーンを指定しているので、
Visual Studio Build Tools も要りません。

```bash
cargo build --release
```

```bash
./target/release/LightTaskManager.exe
```

## 操作

| 操作 | キー / マウス |
| --- | --- |
| フィルタ | タイトルバーの検索欄、`Ctrl+F`、または表にフォーカスがある状態で文字を入力 |
| フィルタ解除 | 検索欄で `Esc` |
| ソート | 列見出しをクリック |
| 列の幅 / 順序 | 見出しの境界をドラッグ / 見出しをドラッグ |
| 列の選択 | ツールバーの列ボタン、または見出しを右クリック |
| 行の移動 | `↑` `↓` `PageUp` `PageDown` `Home` `End` |
| タスクの終了 | `Delete`、ツールバーのボタン、または行を右クリック |
| 行をコピー | `Ctrl+C` |
| 今すぐ更新 | `F5` |

## 構成

| ファイル | 役割 |
| --- | --- |
| `src/sampler.rs` | バックグラウンドのプロセス情報取得 |
| `src/columns.rs` | 列の定義、整形、比較 |
| `src/gfx.rs` | CPU 描画（DIB への塗りと GDI の文字描画） |
| `src/theme.rs` | 配色 |
| `src/core.rs` | タイトルバー、サイドバー、ページの切り替え |
| `src/window.rs` | Win32 メッセージの処理 |
| `src/pages/` | 各ページ |

## ページの追加

1. `src/pages/` に `Page` トレイトを実装したファイルを作ります。
2. `src/pages/mod.rs` の `all()` に 1 行追加します。サイドバーの項目は `Page::nav()` から自動で作られます。

## 開発用コマンド

```bash
./target/release/LightTaskManager.exe --bench bench.txt
```

```bash
./target/release/LightTaskManager.exe --shot shot.bmp 0
```

`--bench` は 5,000 行での描画、スクロール、フィルタ、ソートの時間を計測します。
`--shot` はウィンドウを開かずに指定ページ（0 始まり）を BMP に書き出します。
