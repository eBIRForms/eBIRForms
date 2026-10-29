//! Notices, announcements, tax deadlines, and penalty cache repository.

use rusqlite::params;

use super::{
    Announcement, BirNotice, Database, DbError, NoticeSourceKind, NoticeType, PenaltyCache,
    TaxDeadline,
};

impl Database {
    // =========================================================================
    // Tax Deadlines
    // =========================================================================
    pub fn save_tax_deadline(&self, deadline: &TaxDeadline) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO tax_deadlines (form_type, due_date, description) VALUES (?1, ?2, ?3)",
            params![deadline.form_type, deadline.due_date, deadline.description],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_tax_deadlines(&self) -> Result<Vec<TaxDeadline>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, form_type, due_date, description FROM tax_deadlines ORDER BY due_date ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TaxDeadline {
                id: row.get(0)?,
                form_type: row.get(1)?,
                due_date: row.get(2)?,
                description: row.get(3)?,
            })
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }

    // =========================================================================
    // Notices and Announcements
    // =========================================================================
    pub fn save_bir_notice(&self, notice: &BirNotice) -> Result<i64, DbError> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO bir_notices (external_id, source, source_kind, source_url, title, body, notice_type, rdo_code, form_code, deadline, image_url, posted_at, raw_json, read_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(source_kind, external_id) DO UPDATE SET
                 title=excluded.title,
                 body=excluded.body,
                 source_url=excluded.source_url,
                 notice_type=excluded.notice_type,
                 posted_at=excluded.posted_at,
                 raw_json=excluded.raw_json,
                 fetched_at=datetime('now')"
        )?;

        let read_status = if notice.read_status { 1 } else { 0 };
        stmt.execute(params![
            notice.external_id,
            notice.source,
            notice.source_kind.as_str(),
            notice.source_url,
            notice.title,
            notice.body,
            notice.notice_type.as_str(),
            notice.rdo_code,
            notice.form_code,
            notice.deadline,
            notice.image_url,
            notice.posted_at,
            notice.raw_json,
            read_status
        ])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_bir_notices(&self) -> Result<Vec<BirNotice>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, external_id, source, source_kind, source_url, title, body, notice_type, rdo_code, form_code, deadline, image_url, posted_at, fetched_at, raw_json, read_status
             FROM bir_notices ORDER BY posted_at DESC, id DESC LIMIT 50",
        )?;

        let notices = stmt
            .query_map([], |row| {
                let kind_str: String = row.get(3)?;
                let type_str: String = row.get(7)?;
                Ok(BirNotice {
                    id: row.get(0)?,
                    external_id: row.get(1)?,
                    source: row.get(2)?,
                    source_kind: NoticeSourceKind::from_string(&kind_str),
                    source_url: row.get(4)?,
                    title: row.get(5)?,
                    body: row.get(6)?,
                    notice_type: NoticeType::from_string(&type_str),
                    rdo_code: row.get(8)?,
                    form_code: row.get(9)?,
                    deadline: row.get(10)?,
                    image_url: row.get(11)?,
                    posted_at: row.get(12)?,
                    fetched_at: row.get(13)?,
                    raw_json: row.get(14)?,
                    read_status: row.get::<_, i32>(15)? != 0,
                })
            })?
            .filter_map(Result::ok)
            .collect();

        Ok(notices)
    }

    pub fn save_announcement(&self, ann: &Announcement) -> Result<i64, DbError> {
        self.save_bir_notice(&BirNotice {
            id: None,
            external_id: format!(
                "legacy-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_micros()
            ),
            source: ann.source.clone(),
            source_kind: NoticeSourceKind::Rss,
            source_url: None,
            title: ann.title.clone(),
            body: ann.content.clone(),
            notice_type: NoticeType::General,
            rdo_code: None,
            form_code: None,
            deadline: None,
            image_url: None,
            posted_at: Some(ann.published_at.clone()),
            fetched_at: "now".to_string(),
            raw_json: None,
            read_status: ann.read_status,
        })
    }

    pub fn list_announcements(&self) -> Result<Vec<Announcement>, DbError> {
        let notices = self.list_bir_notices()?;
        Ok(notices
            .into_iter()
            .map(|n| Announcement {
                id: n.id,
                source: n.source,
                title: n.title,
                content: n.body,
                published_at: n.posted_at.unwrap_or_default(),
                read_status: n.read_status,
            })
            .collect())
    }

    // =========================================================================
    // Penalties Cache
    // =========================================================================
    pub fn save_penalty_cache(&self, cache: &PenaltyCache) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO penalties_cache (tin, form_type, period, penalty_amount, reason, is_high_risk) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![cache.tin, cache.form_type, cache.period, cache.penalty_amount, cache.reason, cache.is_high_risk],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_penalties_cache(&self, tin: &str) -> Result<Vec<PenaltyCache>, DbError> {
        let mut stmt = self.conn.prepare("SELECT id, tin, form_type, period, penalty_amount, reason, is_high_risk, calculated_at FROM penalties_cache WHERE tin = ?1 ORDER BY calculated_at DESC")?;
        let rows = stmt.query_map(params![tin], |row| {
            Ok(PenaltyCache {
                id: row.get(0)?,
                tin: row.get(1)?,
                form_type: row.get(2)?,
                period: row.get(3)?,
                penalty_amount: row.get(4)?,
                reason: row.get(5)?,
                is_high_risk: row.get(6)?,
                calculated_at: row.get(7)?,
            })
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }
}
