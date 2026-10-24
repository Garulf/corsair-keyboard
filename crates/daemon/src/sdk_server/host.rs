use engine::ClientLighting;
use ipc::{SdkAccess, SdkClient};
use sdk::{AccessLevel, ClientSummary, DeviceSnapshot, Policy, SdkHost};

use crate::controller::Controller;

impl SdkHost for Controller {
    fn device(&self) -> Option<DeviceSnapshot> {
        self.sdk_device()
    }

    fn policy(&self) -> Policy {
        let settings = self.sdk_settings();
        Policy {
            enabled: settings.enabled,
            exclusive_control: settings.exclusive_control,
            blocked: settings.blocked.clone(),
        }
    }

    fn set_lighting(&mut self, lighting: ClientLighting) {
        self.set_client_lighting(lighting);
    }

    fn set_intercepted(&mut self, leds: Vec<u8>) {
        self.set_sdk_taken_keys(&leds);
    }

    fn clients_changed(&mut self, clients: Vec<ClientSummary>) {
        self.set_sdk_clients(clients.iter().map(sdk_client).collect());
    }
}

fn sdk_client(summary: &ClientSummary) -> SdkClient {
    SdkClient {
        exe: summary.exe.clone(),
        access: access(summary.access),
        priority: summary.priority,
        intercepted: summary.intercepted.iter().map(|key| format!("G{}", key.0)).collect(),
        refused: summary.refused,
    }
}

fn access(level: AccessLevel) -> SdkAccess {
    match level {
        AccessLevel::Shared => SdkAccess::Shared,
        AccessLevel::ExclusiveLighting => SdkAccess::ExclusiveLighting,
        AccessLevel::ExclusiveKeys => SdkAccess::ExclusiveKeys,
        AccessLevel::Exclusive => SdkAccess::Exclusive,
    }
}
