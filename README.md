# WorkStash

Keep notes, prompts, AI skills, n8n workflows and project files together in a local desktop library. The development build targets Fedora KDE Plasma on x86_64 and uses a Russian interface.

## Try the development build

In the **Actions** tab, open a successful **Fedora development build** run on `dev`. Download the **WorkStash-Fedora44-x86_64** artifact while signed in, extract the RPM and install it using the Fedora package installer. Open **WorkStash** from the application menu. There is no stable release yet.

Choose a directory on your disk. WorkStash creates a private `WorkStash.library` folder there and remembers its location. Create a section or use **Входящие** to collect materials for later sorting. New imports from **Все материалы** go to the inbox. Drop a file or folder into the window. Select an entry to read it or locate the saved copy. Under **+ Добавить**, choose **Текстовая запись** for a note and tick **Промпт** for reusable prompt text. Use **Скопировать** in the details to copy the text. Access cards have their own editor. Click outside a panel or press Escape to close it. Reopen the same editor to continue a draft after closing the panel or restarting WorkStash. Tabs filter the selected section by material type. **Настройки → Тема оформления** follows the desktop by default and remembers an explicit light or dark choice.

Open an item and choose **Переместить в раздел…** to move it. For several items, click **Выбрать**, tick individual rows or **Все в списке**, then choose the destination. Selection applies to the current search and type filter.

## Imports and search

- File and folder imports copy the original, including a skill folder with its `SKILL.md`. Symlinks and special files are rejected.
- Search covers titles, types, notes, prompts, workflow node and credential names, and text previews. CSV, Markdown and plain-text previews include the first 64 KiB. Local single-workflow JSON exports up to 32 MiB get a workflow summary. Other formats are kept as files.
- In **+ Добавить → Из n8n…**, enter your server's HTTPS address and API key. Choose workflows or data tables, load the list and select an item to save an offline JSON snapshot. Table snapshots include metadata and rows. Re-selecting an item from that server updates its catalog entry. Select a saved table to browse its columns and rows offline. CSV imports use the same paged viewer. Older snapshot files remain on disk. Data table access requires a server version exposing the public data table endpoints.
- Create a **Карточка доступа** entry using the credential name shown in a workflow to see name matches. Identical names across servers can match multiple workflows. To store a password or token, open that card and enter it in the separate secret field. WorkStash uses the desktop Secret Service through `secret-tool`. A working, unlocked Secret Service collection is required. Copying a secret puts it on the system clipboard.

## Storage

The catalog and imported files stay on the selected disk. Workflow JSON and data table snapshots retain their original contents and may contain sensitive fields. Treat the library as private. Back up the whole library folder with the app closed. Manually entered secrets are stored separately in your desktop keyring, which needs its own backup. The n8n API key stays in memory while its connection dialog is open.

## License

GNU General Public License v3. See [LICENSE](LICENSE).
