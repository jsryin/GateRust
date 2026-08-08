use std::time::Duration;

use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

use crate::{
    Result, TunnelError,
    client::ClientTunnel,
    config::{ClientServiceConfig, validate_client_services},
};

const CLIENT_COMMAND_CAPACITY: usize = 4;
pub(crate) const SERVICE_UPDATE_TIMEOUT: Duration = Duration::from_secs(20);

/// 向正在运行的客户端控制会话提交临时隧道选择。
#[derive(Clone)]
pub struct ClientController {
    sender: mpsc::Sender<ClientCommand>,
}

/// 客户端后台任务持有的控制命令接收端。
pub struct ClientCommandReceiver {
    pub(crate) receiver: mpsc::Receiver<ClientCommand>,
}

pub(crate) type CommandResponse = oneshot::Sender<std::result::Result<Vec<ClientTunnel>, String>>;

pub(crate) struct ClientCommand {
    pub(crate) services: Vec<ClientServiceConfig>,
    pub(crate) response: CommandResponse,
    pub(crate) deadline: Instant,
}

/// 创建桌面运行时使用的有界客户端控制通道。
#[must_use]
pub fn client_control_channel() -> (ClientController, ClientCommandReceiver) {
    let (sender, receiver) = mpsc::channel(CLIENT_COMMAND_CAPACITY);
    (
        ClientController { sender },
        ClientCommandReceiver { receiver },
    )
}

impl ClientController {
    /// 应用临时服务列表，并等待服务端返回应用后的隧道快照。
    ///
    /// # Errors
    ///
    /// 服务配置无效、客户端任务已停止、控制连接中断或服务端拒绝请求时返回错误。
    pub async fn update_services(
        &self,
        services: Vec<ClientServiceConfig>,
    ) -> Result<Vec<ClientTunnel>> {
        validate_client_services(&services)?;
        self.update_services_until(services, Instant::now() + SERVICE_UPDATE_TIMEOUT)
            .await
    }

    async fn update_services_until(
        &self,
        services: Vec<ClientServiceConfig>,
        deadline: Instant,
    ) -> Result<Vec<ClientTunnel>> {
        let (response, result) = oneshot::channel();
        tokio::time::timeout_at(deadline, async {
            self.sender
                .send(ClientCommand {
                    services,
                    response,
                    deadline,
                })
                .await
                .map_err(|_| TunnelError::Protocol("客户端控制任务已停止".into()))?;
            result
                .await
                .map_err(|_| TunnelError::Protocol("客户端控制请求未完成".into()))?
                .map_err(TunnelError::Protocol)
        })
        .await
        .map_err(|_| TunnelError::Timeout("等待客户端应用服务配置"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_deadline_includes_queue_wait() {
        let (controller, mut commands) = client_control_channel();
        let result = controller
            .update_services_until(Vec::new(), Instant::now() + Duration::from_millis(10))
            .await;

        assert!(matches!(result, Err(TunnelError::Timeout(_))));
        let command = commands.receiver.recv().await.expect("命令应已进入队列");
        assert!(command.response.is_closed());
    }
}
