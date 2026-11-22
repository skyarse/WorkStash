import base64
import importlib.util
import io
import os
from pathlib import Path
import sqlite3
import tarfile
import tempfile
import unittest

os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')


def module(name):
    path = Path(__file__).resolve().parents[1] / 'src' / 'joplin' / f'{name}.py'
    spec = importlib.util.spec_from_file_location('ws_' + name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


importer = module('import')
reader = module('reader')
app = reader.QApplication([])
FOLDER, CHILD, NOTE, NEXT, TAG, REL, RES = [f'{i:032x}' for i in range(1, 8)]


def serialized(item, kind, title='', body='', **props):
    fields = dict(id=item, type_=str(kind), **props)
    return '\n\n'.join(part for part in (title, body, '\n'.join(f'{k}: {v}' for k, v in fields.items())) if part).encode()


def make_archive(path, entries):
    with tarfile.open(path, 'w') as tar:
        for name, data in entries.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            tar.addfile(entry, io.BytesIO(data))


class ImportTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'WorkStash.library'
        (self.root / 'files').mkdir(parents=True)
        self.db = sqlite3.connect(self.root / 'catalog.sqlite3')
        self.addCleanup(self.db.close)
        self.db.executescript("""CREATE TABLE sections(id INTEGER PRIMARY KEY,name TEXT UNIQUE);
            INSERT INTO sections VALUES(1,'Входящие');
            CREATE TABLE entries(id INTEGER PRIMARY KEY,section INTEGER,kind TEXT,title TEXT,body TEXT,source TEXT UNIQUE);""")
        self.archive = self.root.parent / 'test.jex'
        self.body = f'## Заголовок\n\n---\n\n[Следующая](:/{NEXT})\n\n![Пиксель](:/{RES})\n\n```\npwd\n```'
        self.members = {
            CHILD+'.md': serialized(CHILD, 2, 'Сервер', parent_id=FOLDER),
            FOLDER+'.md': serialized(FOLDER, 2, 'Дом'),
            NOTE+'.md': serialized(NOTE, 1, 'Команды', self.body, parent_id=CHILD, updated_time='2020-01-01'),
            NEXT+'.md': serialized(NEXT, 1, 'HTML', '<p style="font-family:Serif; margin-left:20px"><b>Стиль</b></p>',
                                    parent_id=CHILD, markup_language='2', is_todo='1'),
            TAG+'.md': serialized(TAG, 5, 'Linux'),
            REL+'.md': serialized(REL, 6, note_id=NOTE, tag_id=TAG),
            RES+'.md': serialized(RES, 4, 'dot.png', file_extension='png'),
            'resources/'+RES+'.png': base64.b64decode('R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw=='),
        }

    def test_round_trip_fields_hierarchy_and_reader(self):
        make_archive(self.archive, self.members)
        importer.import_jex(self.root, self.archive)
        self.assertIn('уже', importer.import_jex(self.root, self.archive))
        entry, section, body = self.db.execute("""SELECT e.id,s.name,e.body FROM entries e
            JOIN sections s ON s.id=e.section WHERE title='Команды'""").fetchone()
        self.assertEqual((section, body), ('Дом › Сервер', self.body))
        window = reader.Reader(self.root, entry)
        self.addCleanup(window.close)
        self.assertEqual(window.fields['tags'], ['Linux'])
        self.assertEqual(window.fields['updated_time'], '2020-01-01')
        self.assertEqual(window.attachment(RES).read_bytes(), self.members['resources/'+RES+'.png'])
        image = window.browser.document().resource(reader.QTextDocument.ResourceType.ImageResource, reader.QUrl('joplin:'+RES))
        self.assertEqual(image.width(), 1)
        self.assertIn('pwd', window.browser.toPlainText())
        self.assertIn('joplin:'+NEXT, window.browser.toHtml())
        window.follow(reader.QUrl('joplin:'+NEXT))
        self.assertIn('Стиль', window.browser.toPlainText())
        self.assertIn('Serif', window.browser.toHtml())
        self.assertIn('Задача', window.browser.toPlainText())
        window.go_back()
        self.assertEqual(window.fields['title'], 'Команды')
        saved = self.db.execute('SELECT archive FROM joplin_imports').fetchone()[0]
        self.assertEqual((self.root / saved).read_bytes(), self.archive.read_bytes())

    def test_invalid_archive_rolls_back(self):
        self.members['../bad'] = b'invalid'
        make_archive(self.archive, self.members)
        with self.assertRaises(ValueError):
            importer.import_jex(self.root, self.archive)
        self.assertEqual(self.db.execute('SELECT count(*) FROM entries').fetchone()[0], 0)
        self.assertEqual(list((self.root / 'files').iterdir()), [])

    def test_missing_attachment_reported(self):
        del self.members['resources/'+RES+'.png']
        make_archive(self.archive, self.members)
        self.assertIn('отсутствуют файлы вложений: 1', importer.import_jex(self.root, self.archive))


if __name__ == '__main__':
    unittest.main()
