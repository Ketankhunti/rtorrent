/// Represents all possible errors that can occur during Bencode parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BencodeError {
   
    UnexpectedEof,
    InvalidType,
    TrailingData,
    
    // --- Integer-specific Errors ---
    InvalidIntegerFormat,
    IntegerLeadingZero,
    IntegerEmpty,

    // --- String-specific Errors ---
    StringMissingColon,
    StringInvalidLength,

    // --- Dictionary-specific Errors ---
    DictKeyNotString,
    DictKeysNotSorted,

    // --- emantic Errors ---
    RootNotADictionary,
    MissingAnnounceURL,
    MissingInfoDict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerError {
    /// The HTTP request to the tracker failed.
    RequestFailed(String),
    /// The tracker responded with a non-200 status code.
    UnsuccessfulResponse(u16),
    /// The tracker sent a failure reason in its response.
    Failure(String),
    /// The 'peers' key was missing from the tracker's response.
    MissingPeers,
    /// The tracker's peer list was not a multiple of 6 (for IPv4) or 18 (for IPv6).
    InvalidPeerListFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppError {
    Bencode(BencodeError),
    Tracker(TrackerError),
}

impl From<BencodeError> for AppError {
    fn from(e: BencodeError) -> AppError {
        AppError::Bencode(e)
    }
}

impl From<TrackerError> for AppError {
    fn from(e: TrackerError) -> Self {
        AppError::Tracker(e)
    }
}
