# N

[English](#n) | [日本語](#n-1)

**Nostr a simple status sharing application to eliminate conversation.**

## Abstract

In today's social networking services, the focus is on conversation, and it is not uncommon to see conversations that are not desired.
This application was created to bring back the simple "status sharing" of the early days of Twitter, rather than the conversational aspect of Twitter.
This application is a simple GUI application that allows you to post your status using NIP-38.

## Screenshot

(Coming soon)

## Features

*   **Intuitive GUI:** Easy-to-use graphical interface for all operations.
*   **Post Status Updates (NIP-38):** Easily post your current status.
*   **Secure Key Management (NIP-49):** Your secret key is encrypted with a passphrase and stored locally. You will be prompted for the passphrase at startup.
*   **Relay Management (NIP-65):** The application automatically discovers your relay list from your NIP-65 event. If you don't have a NIP-65 event, it connects to default relays.
*   **Timeline Display for Follows (NIP-02 & NIP-38):** Displays the latest statuses of users you follow in a timeline format.
*   **Tabbed Interface:** Separate tabs for Home (Timeline & Posting), Relays, and Profile for easy navigation.
*   **No Conversation Features:** This tool is for posting statuses only. There are no replies, mentions, or other conversational features.

## Technical Stacks

*   [rust-nostr](https://docs.rs/nostr/latest/nostr/index.html)
*   [eframe](https://github.com/emilk/egui/tree/master/crates/eframe)
*   [egui](https://github.com/emilk/egui)

## Installation & Usage

1.  **Clone the repository and navigate to the directory:**
    ```bash
    git clone https://github.com/tami1A84/nostr-nip38-status-sender.git
    cd nostr-nip38-status-sender
    ```
2.  **Run the application:**
    ```bash
    cargo run
    ```
3.  **The GUI window will open. Follow the on-screen instructions for setup and status posting.**

    > **Note on Relays (NIP-65):**
    > If you have published a relay list with NIP-65, the application will automatically use those relays for posting. If not, it will connect to default relays.

---

# N

[English](#n) | [日本語](#n-1)

**会話をなくし、シンプルなステータス共有を実現するNostrアプリケーション**

## 概要

現代のSNSは会話が中心となり、望まない会話を目にすることも少なくありません。
このアプリケーションは、Twitterの原点であるシンプルな「ステータス共有」に回帰するために作成されました。
NIP-38を利用してあなたのステータスを投稿するための、シンプルなGUIアプリケーションです。

## スクリーンショット

（準備中）

## 特徴

*   **直感的なGUI:** 全ての操作を簡単に行えるグラフィカルインターフェース。
*   **ステータス投稿 (NIP-38):** あなたの現在の状況を簡単に投稿できます。
*   **安全な鍵管理 (NIP-49):** 秘密鍵はパスフレーズで暗号化され、ローカルに保存されます。起動時にパスフレーズの入力が求められます。
*   **リレー管理 (NIP-65):** あなたが公開しているNIP-65イベントから、リレーリストを自動的に取得します。NIP-65を公開していない場合は、デフォルトのリレーに接続します。
*   **フォローリストのタイムライン表示 (NIP-02 & NIP-38):** フォローしているユーザーの最新ステータスをタイムライン形式で表示します。
*   **タブ形式のインターフェース:** ホーム（タイムラインと投稿）、リレー、プロフィールのタブで簡単に機能を切り替えられます。
*   **会話機能の排除:** このツールはステータス投稿専用です。リプライやメンションなどの会話機能はありません。

## 技術スタック

*   [rust-nostr](https://docs.rs/nostr/latest/nostr/index.html)
*   [eframe](https://github.com/emilk/egui/tree/master/crates/eframe)
*   [egui](https://github.com/emilk/egui)

## インストール & 使い方

1.  **リポジトリをクローンし、ディレクトリに移動します:**
    ```bash
    git clone https://github.com/tami1A84/nostr-nip38-status-sender.git
    cd nostr-nip38-status-sender
    ```
2.  **アプリケーションを実行します:**
    ```bash
    cargo run
    ```
3.  **GUIウィンドウが開きます。画面の指示に従って、初回設定とステータス投稿を行ってください。**

    > **リレーに関する注記 (NIP-65):**
    > もしあなたがNIP-65でリレーリストを公開している場合、アプリケーションは自動的にそのリレーを使用して投稿します。公開していない場合は、デフォルトのリレーに接続されます。