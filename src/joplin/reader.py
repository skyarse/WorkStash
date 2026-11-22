"""Read imported note content and attachments without loading remote resources."""
import html
import json
from pathlib import Path
import re
import shutil
import sqlite3
import sys

import markdown
from markdown.extensions.toc import slugify_unicode
from PyQt6.QtCore import QByteArray, Qt, QUrl
from PyQt6.QtGui import QDesktopServices, QImageReader, QTextDocument
from PyQt6.QtWidgets import (
    QApplication, QDialog, QFileDialog, QHBoxLayout, QLabel, QMainWindow,
    QMessageBox, QPushButton, QTextBrowser, QTextEdit, QVBoxLayout, QWidget,
)

LINK = re.compile(r"""((?:href|src)\s*=\s*["']):/([0-9a-fA-F]{32})(?=["'#])""", re.I)


def render(body, props):
    if props.get('is_locked') == '1' or props.get('encryption_applied') == '1':
        return '<p>Заметка зашифрована. Разблокируйте её в Joplin и экспортируйте повторно.</p>'
    if props.get('markup_language') == '2':
        content = body or props.get('body_html', '')
    else:
        content = markdown.markdown(body, extensions=['tables', 'fenced_code', 'sane_lists', 'nl2br', 'toc'],
                                    extension_configs={'toc': {'slugify': slugify_unicode}})
        content = re.sub(r'(<li>\s*(?:<p>)?)\[([ xX])\]\s', lambda m: m[1] + ('☐ ' if m[2] == ' ' else '☑ '), content)
    return LINK.sub(lambda m: m[1] + 'joplin:' + m[2].lower(), content)


class NoteBrowser(QTextBrowser):
    def __init__(self, reader):
        super().__init__()
        self.reader = reader
        self.setOpenLinks(False)
        self.setOpenExternalLinks(False)
        self.anchorClicked.connect(reader.follow)

    def loadResource(self, kind, url):
        if kind in (QTextDocument.ResourceType.ImageResource, QTextDocument.ResourceType.ImageResource.value) and url.scheme() == 'joplin':
            path = self.reader.attachment(url.path())
            if path:
                image = QImageReader(str(path)).read()
                if not image.isNull():
                    return image
        return QByteArray()


class Reader(QMainWindow):
    def __init__(self, root, entry_id):
        super().__init__()
        self.root = Path(root).resolve()
        self.db = sqlite3.connect((self.root / 'catalog.sqlite3').as_uri() + '?mode=ro', uri=True)
        row = self.db.execute("SELECT batch,item FROM joplin_items WHERE entry_id=?", (entry_id,)).fetchone()
        if not row:
            raise ValueError("Запись Joplin не найдена")
        self.batch, item = row
        self.history = []
        self.resize(900, 720)
        page = QWidget()
        layout = QVBoxLayout(page)
        bar = QHBoxLayout()
        self.back = QPushButton('← Назад')
        self.back.clicked.connect(self.go_back)
        bar.addWidget(self.back)
        self.path = QLabel()
        self.path.setTextFormat(Qt.TextFormat.PlainText)
        bar.addWidget(self.path, 1)
        fields = QPushButton('Поля заметки…')
        fields.clicked.connect(self.show_fields)
        bar.addWidget(fields)
        layout.addLayout(bar)
        self.browser = NoteBrowser(self)
        layout.addWidget(self.browser)
        self.setCentralWidget(page)
        self.show_note(item)

    def attachment(self, item):
        row = self.db.execute("SELECT asset FROM joplin_items WHERE batch=? AND item=? AND kind=4",
                              (self.batch, item)).fetchone()
        if row and row[0]:
            path = (self.root / row[0]).resolve()
            if path.is_relative_to(self.root / 'files') and path.is_file():
                return path
        return None

    def show_note(self, item, remember=True):
        row = self.db.execute("""SELECT e.title,e.body,i.props,s.name FROM joplin_items i
            JOIN entries e ON e.id=i.entry_id JOIN sections s ON s.id=e.section
            WHERE i.batch=? AND i.item=?""", (self.batch, item)).fetchone()
        if not row:
            return
        title, body, raw, section = row
        self.fields = json.loads(raw)
        tags = self.db.execute("""SELECT t.title FROM joplin_items rel
            JOIN joplin_items t ON t.batch=rel.batch AND t.item=json_extract(rel.props,'$.tag_id')
            WHERE rel.batch=? AND rel.kind=6 AND json_extract(rel.props,'$.note_id')=? ORDER BY t.title""",
            (self.batch, item)).fetchall()
        if tags:
            self.fields['tags'] = [r[0] for r in tags]
        self.path.setText(section)
        self.setWindowTitle(f'{title} — WorkStash')
        self.fields['title'] = title
        info = ' · '.join(html.escape(r[0]) for r in tags)
        if self.fields.get('is_todo') == '1':
            done = self.fields.get('todo_completed', '0') not in ('0', '')
            info = ('☑ Выполнено' if done else '☐ Задача') + (' · ' + info if info else '')
        self.browser.setHtml(f'<h1>{html.escape(title)}</h1><p>{info}</p>' + render(body, self.fields))
        if remember:
            self.history.append(item)
        self.back.setEnabled(len(self.history) > 1)

    def go_back(self):
        if len(self.history) > 1:
            self.history.pop()
            self.show_note(self.history[-1], remember=False)

    def follow(self, url):
        if url.scheme() in ('https', 'http', 'mailto'):
            QDesktopServices.openUrl(url)
            return
        if not url.scheme() and url.fragment() and not url.path():
            self.browser.scrollToAnchor(url.fragment())
            return
        if url.scheme() != 'joplin':
            return
        item = url.path()
        row = self.db.execute("SELECT kind,title,props FROM joplin_items WHERE batch=? AND item=?",
                              (self.batch, item)).fetchone()
        if row and row[0] == 1:
            self.show_note(item)
            if url.fragment():
                self.browser.scrollToAnchor(url.fragment())
            return
        path = self.attachment(item)
        if not path:
            QMessageBox.information(self, 'WorkStash', 'Цель ссылки отсутствует в этом экспорте.')
            return
        if path.suffix.lower() in ('.png', '.jpg', '.jpeg', '.gif', '.webp', '.pdf', '.txt'):
            QDesktopServices.openUrl(QUrl.fromLocalFile(str(path)))
        else:
            name = Path(json.loads(row[2]).get('filename') or row[1] or path.name).name
            dest, _ = QFileDialog.getSaveFileName(self, 'Сохранить вложение', name)
            if dest:
                try:
                    shutil.copyfile(path, dest)
                except OSError as err:
                    QMessageBox.warning(self, 'WorkStash', str(err))

    def show_fields(self):
        dialog = QDialog(self)
        dialog.setWindowTitle('Поля заметки')
        dialog.resize(620, 480)
        layout = QVBoxLayout(dialog)
        text = QTextEdit()
        text.setReadOnly(True)
        text.setPlainText('\n'.join(f'{key}: {value}' for key, value in self.fields.items() if value != ''))
        layout.addWidget(text)
        dialog.exec()


if __name__ == '__main__':
    app = QApplication(sys.argv[:1])
    app.setApplicationName('WorkStash')
    viewer = Reader(sys.argv[1], int(sys.argv[2]))
    viewer.show()
    sys.exit(app.exec())
