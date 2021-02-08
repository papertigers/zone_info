use thiserror::Error;

mod kstat;

#[derive(Error, Debug)]
pub enum ZoneInfoError {
    #[error("kstat lookup failed")]
    Kstat(#[from] crate::kstat::KstatError),
    #[error("io error")]
    IOError(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, ZoneInfoError>;

pub fn zone_cpus() -> Result<usize> {
    if let Some(cap) = kstat::zone_cpu_cap()? {
        return Ok((cap / 100) as usize);
    }

    kstat::ncpus().map_err(ZoneInfoError::from)
}

pub fn zoneid() -> Result<i32> {
    Ok(zonename::getzoneid()?)
}
