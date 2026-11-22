"""Import Joplin's serialized items and resources into a WorkStash library."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import sqlite3
import sys
import tarfile
import tempfile

ID = r"[a-fA-F0-9]{32}"
OBJECT = re.compile(rf"({ID})\.md")
RESOURCE = re.compile(rf"resources/({ID})(?:\.[^/]+)?")
SCHEMA = (
    "CREATE TABLE IF NOT EXISTS joplin_imports(batch TEXT PRIMARY KEY, archive TEXT NOT NULL)",
    """CREATE TABLE IF NOT EXISTS joplin_items(
        batch TEXT NOT NULL REFERENCES joplin_imports(batch), item TEXT NOT NULL,
        kind INTEGER NOT NULL, title TEXT NOT NULL, props TEXT NOT NULL, asset TEXT,
        entry_id INTEGER REFERENCES entries(id) ON DELETE SET NULL,
        section_id INTEGER REFERENCES sections(id) ON DELETE SET NULL, PRIMARY KEY(batch,item))""",
    "CREATE INDEX IF NOT EXISTS joplin_entry ON joplin_items(entry_id)",
)


def readable(name, fallback):
    clean = re.sub(r'[/\\\x00-\x1f\x7f]', ' ', name).strip(' .')
    return clean[:45] or fallback


def parse_item(content, expected):
    lines = content.replace('\r\n', '\n').rstrip('\n').split('\n')
    props = {}
    while lines and lines[-1].strip():
        key, sep, value = lines.pop().partition(':')
        if not sep:
            raise ValueError(f"Некорректное поле записи {expected}")
        # Joplin escapes newlines only in metadata, not in the body.
        props[key.strip()] = re.sub(r'(\\*)\\([nr])', lambda m: m[0][1:] if m[1]
                                    else ('\n' if m[2] == 'n' else '\r'), value.strip())
    if props.get('id', '').lower() != expected or not props.get('type_', '').isdigit():
        raise ValueError(f"Некорректный идентификатор или тип записи {expected}")
    if lines:
        lines.pop()
    return int(props['type_']), lines[0] if lines else '', '\n'.join(lines[2:]), props


def catalog_archive(db, batch, stage, root):
    folders, notes, binaries = {}, [], {}
    db.execute("CREATE TEMP TABLE joplin_bodies(item TEXT PRIMARY KEY, body TEXT)")
    members = set()
    archive_size = (stage / 'original.jex').stat().st_size
    with tarfile.open(stage / 'original.jex', mode='r|') as archive:
        for member in archive:
            path = PurePosixPath(member.name)
            if path.is_absolute() or '..' in path.parts or not (member.isdir() or member.isfile()):
                raise ValueError("Недопустимый путь или тип файла внутри JEX")
            if member.isdir():
                continue
            if member.sparse or member.size > archive_size:
                raise ValueError("Недопустимый размер файла внутри JEX")
            name = str(path)
            if name in members:
                raise ValueError(f"Повторяющийся файл в архиве: {name}")
            members.add(name)
            match = OBJECT.fullmatch(name)
            if match:
                if member.size > 64 * 1024**2:
                    raise ValueError("Отдельная текстовая запись превышает 64 МиБ")
                item = match[1].lower()
                kind, title, body, props = parse_item(archive.extractfile(member).read().decode('utf-8'), item)
                db.execute("INSERT INTO joplin_items(batch,item,kind,title,props) VALUES(?,?,?,?,?)",
                           (batch, item, kind, title, json.dumps(props, ensure_ascii=False)))
                if kind == 1:
                    notes.append((item, title, props.get('parent_id', '').lower()))
                    db.execute("INSERT INTO joplin_bodies VALUES(?,?)", (item, body))
                elif kind == 2:
                    folders[item] = (title or 'Без названия', props.get('parent_id', '').lower())
                continue
            match = RESOURCE.fullmatch(name)
            if match:
                item = match[1].lower()
                if item in binaries:
                    raise ValueError(f"Два вложения с одним ID: {item}")
                dest = stage / 'Вложения' / item
                dest.parent.mkdir(exist_ok=True)
                with dest.open('xb') as out:
                    shutil.copyfileobj(archive.extractfile(member), out)
                binaries[item] = dest

    if not notes and not folders and not db.execute(
            "SELECT 1 FROM joplin_items WHERE batch=? LIMIT 1", (batch,)).fetchone():
        raise ValueError("В архиве не найдены данные Joplin")
    resources = db.execute("SELECT item,title,props FROM joplin_items WHERE batch=? AND kind=4",
                           (batch,)).fetchall()
    missing = 0
    for item, title, raw in resources:
        source = binaries.get(item)
        if source is None:
            missing += 1
            continue
        props = json.loads(raw)
        filename = readable(props.get('filename') or title, 'Вложение')
        suffix = Path(filename).suffix or ('.' + props['file_extension'] if props.get('file_extension') else '')
        suffix = suffix if re.fullmatch(r'\.[a-zA-Z0-9]{1,12}', suffix) else ''
        dest = source.with_name(f"{Path(filename).stem} — {item}{suffix}")
        source.rename(dest)
        db.execute("UPDATE joplin_items SET asset=? WHERE batch=? AND item=?",
                   (str(dest.relative_to(root)), batch, item))

    sections = {}
    def add_section(label):
        name, i = label, 1
        while db.execute("SELECT 1 FROM sections WHERE name=?", (name,)).fetchone():
            i += 1
            name = f"{label} ({i})"
        ident = db.execute("INSERT INTO sections(name) VALUES(?)", (name,)).lastrowid
        return ident, name

    def section_for(item):
        chain = []
        seen = set()
        while item in folders and item not in sections:
            if item in seen:
                raise ValueError("Циклическая вложенность блокнотов Joplin")
            seen.add(item)
            chain.append(item)
            item = folders[item][1]
        parent = sections.get(item)
        for child in reversed(chain):
            title = folders[child][0]
            parent = add_section(f"{parent[1]} › {title}" if parent else title)
            sections[child] = parent
            db.execute("UPDATE joplin_items SET section_id=? WHERE batch=? AND item=?",
                       (parent[0], batch, child))
        return parent[0] if parent else 1

    for item in folders:
        section_for(item)
    for item, title, parent in notes:
        dest = section_for(parent)
        cursor = db.execute("""INSERT INTO entries(section,kind,title,body,source)
            SELECT ?, 'Joplin', ?, body, ? FROM joplin_bodies WHERE item=?""",
            (dest, title or 'Без названия', f'joplin:{batch}:{item}', item))
        db.execute("UPDATE joplin_items SET entry_id=? WHERE batch=? AND item=?",
                   (cursor.lastrowid, batch, item))
    counts = f"Блокноты: {len(folders)}, заметки: {len(notes)}, вложения: {len(resources)-missing}."
    if missing:
        counts += f" В архиве отсутствуют файлы вложений: {missing}."
    return counts


def import_jex(root, source):
    root, source = Path(root).resolve(), Path(source)
    if source.is_symlink() or not source.is_file() or source.resolve().is_relative_to(root):
        raise ValueError("Выберите файл JEX вне библиотеки")
    with sqlite3.connect(root / 'catalog.sqlite3', timeout=30) as db:
        db.execute("PRAGMA foreign_keys=ON")
        stage = Path(tempfile.mkdtemp(prefix=f"Joplin — {readable(source.stem, 'Архив')} — ", dir=root / 'files'))
        committed = False
        try:
            with source.open('rb') as inp, (stage / 'original.jex').open('xb') as out:
                digest = hashlib.sha256()
                while chunk := inp.read(1024 * 1024):
                    out.write(chunk)
                    digest.update(chunk)
            batch = digest.hexdigest()
            db.execute("BEGIN IMMEDIATE")
            for statement in SCHEMA:
                db.execute(statement)
            if db.execute("SELECT 1 FROM joplin_imports WHERE batch=?", (batch,)).fetchone():
                db.rollback()
                return "Этот архив Joplin уже импортирован."
            db.execute("INSERT INTO joplin_imports VALUES(?,?)",
                       (batch, str((stage / 'original.jex').relative_to(root))))
            result = catalog_archive(db, batch, stage, root)
            db.commit()
            committed = True
            return result
        finally:
            if not committed:
                shutil.rmtree(stage)


if __name__ == '__main__':
    try:
        print(import_jex(*sys.argv[1:]))
    except Exception as error:
        sys.exit(f"Не удалось импортировать JEX: {error}")
