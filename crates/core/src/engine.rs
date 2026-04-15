use futures::channel::mpsc;

pub enum EngineEvent {
    BatchedEvents {
        sub_id: String,
        data: Vec<u8>,
    },
    RelayStatus {
        url: String,
        status: crate::traits::TransportStatus,
    },
    SignerReady {
        signer_type: String,
        pubkey: String,
    },
}

pub struct NostrEngine {
    pub cmd_tx: mpsc::Sender<EngineCommand>,
}

pub enum EngineCommand {
    Subscribe {
        /* TODO */
    },
    Unsubscribe {
        sub_id: String,
    },
    Publish {
        /* TODO */
    },
    SetSigner {
        /* TODO */
    },
    SignEvent {
        /* TODO */
    },
    GetPublicKey,
}
