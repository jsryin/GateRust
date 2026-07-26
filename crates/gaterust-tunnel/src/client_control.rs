use tokio::sync::{mpsc, oneshot};

use crate::{
    Result, TunnelError,
    client::ClientTunnel,
    config::{ClientServiceConfig, validate_client_services},
};

const CLIENT_COMMAND_CAPACITY: usize = 4;

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
        let (response, result) = oneshot::channel();
        self.sender
            .send(ClientCommand { services, response })
            .await
            .map_err(|_| TunnelError::Protocol("客户端控制任务已停止".into()))?;
        result
            .await
            .map_err(|_| TunnelError::Protocol("客户端控制请求未完成".into()))?
            .map_err(TunnelError::Protocol)
    }
}
