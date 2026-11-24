pub const PROTO_VERSION: u16 = 1;
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

const T_HELLO: u8 = 1;
const T_HELLO_OK: u8 = 2;
const T_SNAPSHOT_REQ: u8 = 3;
const T_SNAPSHOT: u8 = 4;
const T_CALL: u8 = 5;
const T_REPLY: u8 = 6;
const T_ERROR: u8 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Hello { proto: u16, token: [u8; 32] },
    HelloOk { proto: u16 },
    SnapshotReq,
    Snapshot { json: Vec<u8> },
    Call { seq: u32, iface: u16, slot: u16, payload: Vec<u8> },
    Reply { seq: u32, status: i32, payload: Vec<u8> },
    Error { code: i32, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    Incomplete,
    TooLarge(u32),
    UnknownTag(u8),
    Malformed,
}

pub fn encode(frame: &Frame) -> Vec<u8> {
    let mut body = Vec::new();
    match frame {
        Frame::Hello { proto, token } => {
            body.push(T_HELLO);
            body.extend_from_slice(&proto.to_le_bytes());
            body.extend_from_slice(token);
        }
        Frame::HelloOk { proto } => {
            body.push(T_HELLO_OK);
            body.extend_from_slice(&proto.to_le_bytes());
        }
        Frame::SnapshotReq => body.push(T_SNAPSHOT_REQ),
        Frame::Snapshot { json } => {
            body.push(T_SNAPSHOT);
            body.extend_from_slice(json);
        }
        Frame::Call { seq, iface, slot, payload } => {
            body.push(T_CALL);
            body.extend_from_slice(&seq.to_le_bytes());
            body.extend_from_slice(&iface.to_le_bytes());
            body.extend_from_slice(&slot.to_le_bytes());
            body.extend_from_slice(payload);
        }
        Frame::Reply { seq, status, payload } => {
            body.push(T_REPLY);
            body.extend_from_slice(&seq.to_le_bytes());
            body.extend_from_slice(&status.to_le_bytes());
            body.extend_from_slice(payload);
        }
        Frame::Error { code, message } => {
            body.push(T_ERROR);
            body.extend_from_slice(&code.to_le_bytes());
            body.extend_from_slice(message.as_bytes());
        }
    }
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub fn decode(buf: &[u8]) -> Result<(Frame, usize), ProtoError> {
    if buf.len() < 4 {
        return Err(ProtoError::Incomplete);
    }
    let len = u32_at(buf, 0);
    if len > MAX_FRAME {
        return Err(ProtoError::TooLarge(len));
    }
    if len == 0 {
        return Err(ProtoError::Malformed);
    }
    let total = 4 + len as usize;
    if buf.len() < total {
        return Err(ProtoError::Incomplete);
    }
    let body = &buf[4..total];
    let rest = |from: usize| body[from..].to_vec();
    let frame = match body[0] {
        T_HELLO if body.len() == 35 => {
            let mut token = [0u8; 32];
            token.copy_from_slice(&body[3..35]);
            Frame::Hello { proto: u16_at(body, 1), token }
        }
        T_HELLO_OK if body.len() == 3 => Frame::HelloOk { proto: u16_at(body, 1) },
        T_SNAPSHOT_REQ if body.len() == 1 => Frame::SnapshotReq,
        T_SNAPSHOT => Frame::Snapshot { json: rest(1) },
        T_CALL if body.len() >= 9 => Frame::Call {
            seq: u32_at(body, 1),
            iface: u16_at(body, 5),
            slot: u16_at(body, 7),
            payload: rest(9),
        },
        T_REPLY if body.len() >= 9 => Frame::Reply {
            seq: u32_at(body, 1),
            status: u32_at(body, 5) as i32,
            payload: rest(9),
        },
        T_ERROR if body.len() >= 5 => Frame::Error {
            code: u32_at(body, 1) as i32,
            message: String::from_utf8(rest(5)).map_err(|_| ProtoError::Malformed)?,
        },
        T_HELLO | T_HELLO_OK | T_SNAPSHOT_REQ | T_CALL | T_REPLY | T_ERROR => return Err(ProtoError::Malformed),
        t => return Err(ProtoError::UnknownTag(t)),
    };
    Ok((frame, total))
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
