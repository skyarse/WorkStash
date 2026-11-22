use anyhow::{anyhow, ensure, Context, Result};
use serde_json::{json, Value};
use std::{collections::HashSet, io::Read, time::Duration};
use url::Url;

#[derive(Clone, Copy, PartialEq)]
pub enum Resource {
    Workflow,
    Table,
}
impl Resource {
    fn path(self) -> &'static str {
        match self {
            Self::Workflow => "workflows",
            Self::Table => "data-tables",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Workflow => "Workflow",
            Self::Table => "Таблица",
        }
    }
}
pub struct Remote {
    agent: ureq::Agent,
    root: Url,
    key: String,
}
pub struct Choice {
    pub id: String,
    pub name: String,
}
pub struct Snapshot {
    pub kind: Resource,
    pub source: String,
    pub title: String,
    pub value: Value,
}

impl Remote {
    pub fn new(base: &str, key: String) -> Result<Self> {
        let mut root = Url::parse(base.trim()).context("Введите полный HTTPS-адрес сервера")?;
        ensure!(
            root.scheme() == "https"
                && root.host_str().is_some()
                && root.username().is_empty()
                && root.password().is_none()
                && root.query().is_none()
                && root.fragment().is_none(),
            "Нужен HTTPS-адрес n8n без пароля и параметров"
        );
        ensure!(!key.trim().is_empty(), "Введите API key");
        let path = root.path().trim_end_matches('/');
        if !path.ends_with("/api/v1") {
            root.set_path(&format!("{path}/api/v1"))
        }
        Ok(Self {
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(Duration::from_secs(30))
                .build(),
            root,
            key,
        })
    }
    fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.root.clone();
        for s in segments {
            ensure!(!s.is_empty() && *s != "." && *s != "..", "Некорректный ID");
            url.path_segments_mut()
                .map_err(|_| anyhow!("Некорректный адрес"))?
                .pop_if_empty()
                .push(s);
        }
        Ok(url)
    }
    fn get(&self, url: &Url) -> Result<Value> {
        let r = self
            .agent
            .get(url.as_str())
            .set("X-N8N-API-KEY", &self.key)
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => {
                    anyhow!("n8n: HTTP {code}. Проверьте адрес, права ключа и доступность ресурса.")
                }
                _ => anyhow!("Не удалось подключиться к n8n. Проверьте сеть и сертификат."),
            })?;
        ensure!(
            (200..300).contains(&r.status()),
            "Сервер вернул перенаправление"
        );
        let mut bytes = Vec::new();
        r.into_reader()
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 32 * 1024 * 1024, "Ответ превышает 32 МиБ");
        Ok(serde_json::from_slice(&bytes).context("Некорректный ответ n8n")?)
    }
    fn pages(&self, url: Url) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        let mut total = 0;
        loop {
            let mut page = url.clone();
            page.query_pairs_mut().append_pair("limit", "100");
            if let Some(c) = &cursor {
                page.query_pairs_mut().append_pair("cursor", c);
            }
            let mut response = self.get(&page)?;
            let data = response["data"]
                .as_array_mut()
                .context("В ответе нет списка data")?;
            total += serde_json::to_vec(data)?.len();
            ensure!(total <= 64 * 1024 * 1024, "Размер импорта превышает 64 МиБ");
            rows.append(data);
            match response["nextCursor"].as_str().filter(|s| !s.is_empty()) {
                None => return Ok(rows),
                Some(c) => {
                    ensure!(
                        seen.insert(c.to_owned()) && seen.len() < 2000,
                        "Ошибка постраничной загрузки"
                    );
                    cursor = Some(c.to_owned())
                }
            }
        }
    }
    pub fn list(&self, kind: Resource) -> Result<Vec<Choice>> {
        self.pages(self.endpoint(&[kind.path()])?)?
            .into_iter()
            .map(|v| {
                Ok(Choice {
                    id: v["id"].as_str().context("Нет ID")?.to_owned(),
                    name: v["name"].as_str().context("Нет названия")?.to_owned(),
                })
            })
            .collect()
    }
    pub fn download(&self, kind: Resource, id: &str) -> Result<Snapshot> {
        let url = self.endpoint(&[kind.path(), id])?;
        let meta = self.get(&url)?;
        let title = meta["name"]
            .as_str()
            .context("Нет названия ресурса")?
            .to_owned();
        let value = if kind == Resource::Table {
            json!({"table":meta,"rows":self.pages(self.endpoint(&[kind.path(),id,"rows"])?)?})
        } else {
            ensure!(meta["nodes"].is_array(), "В ответе нет узлов workflow");
            meta
        };
        Ok(Snapshot {
            kind,
            title,
            source: url.to_string(),
            value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn key_stays_on_https_server_and_ids_stay_in_path() -> Result<()> {
        for bad in [
            "http://n8n.example",
            "https://user:password@n8n.example",
            "https://n8n.example/?token=1",
        ] {
            assert!(Remote::new(bad, "dummy".into()).is_err());
        }
        let r = Remote::new("https://n8n.example/sub/api/v1/", "dummy".into())?;
        assert_eq!(
            r.endpoint(&["workflows", "a/b"])?.as_str(),
            "https://n8n.example/sub/api/v1/workflows/a%2Fb"
        );
        assert!(r.endpoint(&[".."]).is_err());
        Ok(())
    }
    #[test]
    fn downloads_all_table_rows_with_get() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let responses = [
            (
                "/api/v1/data-tables?limit=100",
                r#"{"data":[{"id":"t1","name":"Example"}],"nextCursor":null}"#,
            ),
            (
                "/api/v1/data-tables/t1",
                r#"{"id":"t1","name":"Example","columns":[]}"#,
            ),
            (
                "/api/v1/data-tables/t1/rows?limit=100",
                r#"{"data":[{"id":1}],"nextCursor":"A+B"}"#,
            ),
            (
                "/api/v1/data-tables/t1/rows?limit=100&cursor=A%2BB",
                r#"{"data":[{"id":2}],"nextCursor":null}"#,
            ),
        ];
        let server = thread::spawn(move || {
            for (path, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let lines: Vec<_> = BufReader::new(&stream)
                    .lines()
                    .map_while(std::result::Result::ok)
                    .take_while(|l| !l.is_empty())
                    .collect();
                assert_eq!(lines[0], format!("GET {path} HTTP/1.1"));
                assert!(lines
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case("x-n8n-api-key: dummy")));
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let r = Remote {
            root: Url::parse(&format!("http://{address}/api/v1"))?,
            key: "dummy".into(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(3))
                .build(),
        };
        let list = r.list(Resource::Table)?;
        let snapshot = r.download(Resource::Table, &list[0].id)?;
        assert_eq!(snapshot.title, "Example");
        assert_eq!(snapshot.value["rows"][1]["id"], 2);
        server.join().unwrap();
        Ok(())
    }
}
