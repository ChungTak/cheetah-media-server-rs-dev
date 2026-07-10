use cheetah_sdk::{FfmpegApi, FfmpegJob, SdkError};
use dashmap::DashMap;

#[derive(Default)]
pub struct LocalFfmpegService {
    jobs: DashMap<String, FfmpegJob>,
}

impl FfmpegApi for LocalFfmpegService {
    fn submit_job(&self, job: FfmpegJob) -> Result<(), SdkError> {
        if self.jobs.contains_key(&job.job_id) {
            return Err(SdkError::AlreadyExists(format!(
                "ffmpeg job {}",
                job.job_id
            )));
        }
        self.jobs.insert(job.job_id.clone(), job);
        Ok(())
    }

    fn cancel_job(&self, job_id: &str) -> Result<(), SdkError> {
        self.jobs
            .remove(job_id)
            .map(|_| ())
            .ok_or_else(|| SdkError::NotFound(format!("ffmpeg job {job_id}")))
    }

    fn list_jobs(&self) -> Vec<FfmpegJob> {
        let mut out: Vec<_> = self
            .jobs
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        out.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        out
    }
}
