//! Job queue repository — CRUD for background jobs.

use rusqlite::params;

use super::{Database, DbError, Job};

impl Database {
    pub fn save_job(&self, mut job: Job) -> Result<Job, DbError> {
        if let Some(id) = job.id {
            self.conn.execute(
                "UPDATE job_queue SET name = ?1, job_type = ?2, cron_expr = ?3, command = ?4, status = ?5, retries = ?6, last_run_at = ?7, next_run_at = ?8, output_log = ?9 WHERE id = ?10",
                params![job.name, job.job_type, job.cron_expr, job.command, job.status, job.retries, job.last_run_at, job.next_run_at, job.output_log, id],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO job_queue (name, job_type, cron_expr, command, status, retries, last_run_at, next_run_at, output_log) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![job.name, job.job_type, job.cron_expr, job.command, job.status, job.retries, job.last_run_at, job.next_run_at, job.output_log],
            )?;
            job.id = Some(self.conn.last_insert_rowid());

            // Re-fetch to get the created_at timestamp
            let mut stmt = self
                .conn
                .prepare("SELECT created_at FROM job_queue WHERE id = ?1")?;
            if let Ok(mut rows) = stmt.query(params![job.id])
                && let Ok(Some(row)) = rows.next()
            {
                job.created_at = row.get(0).unwrap_or_default();
            }
        }
        Ok(job)
    }

    pub fn list_jobs(&self) -> Result<Vec<Job>, DbError> {
        let mut stmt = self.conn.prepare("SELECT id, name, job_type, cron_expr, command, status, retries, last_run_at, next_run_at, created_at, output_log FROM job_queue ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(Job {
                id: row.get(0)?,
                name: row.get(1)?,
                job_type: row.get(2).unwrap_or_else(|_| "Custom".to_string()),
                cron_expr: row.get(3)?,
                command: row.get(4)?,
                status: row.get(5)?,
                retries: row.get(6)?,
                last_run_at: row.get(7)?,
                next_run_at: row.get(8)?,
                created_at: row.get(9)?,
                output_log: row.get(10).unwrap_or(None),
            })
        })?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row?);
        }
        Ok(jobs)
    }

    pub fn delete_job(&self, id: i64) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM job_queue WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete_archived_jobs(&self) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM job_queue WHERE status = 'Archived'", [])?;
        Ok(())
    }
}
