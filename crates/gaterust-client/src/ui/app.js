const elements = {
  form: document.querySelector('#config-form'),
  notice: document.querySelector('#notice'),
  address: document.querySelector('#server-address'),
  serverName: document.querySelector('#server-name'),
  certificate: document.querySelector('#ca-certificate'),
  key: document.querySelector('#group-key'),
  services: document.querySelector('#services'),
  empty: document.querySelector('#empty-services'),
  count: document.querySelector('#service-count'),
  path: document.querySelector('#config-path'),
  template: document.querySelector('#service-template'),
  statusDot: document.querySelector('#status-dot'),
  statusLabel: document.querySelector('#status-label'),
  statusDetail: document.querySelector('#status-detail')
};

let token = '';
let services = [];
let statusTimer;

async function request(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(options.body ? { 'Content-Type': 'application/json' } : {}),
      ...options.headers
    }
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({ error: `请求失败 (${response.status})` }));
    throw new Error(body.error || `请求失败 (${response.status})`);
  }
  return response.status === 204 ? undefined : response.json();
}

function optional(value) {
  const result = value.trim();
  return result ? result : null;
}

function showNotice(message, kind) {
  elements.notice.textContent = message;
  elements.notice.className = `notice ${kind}`;
  elements.notice.hidden = false;
}

function updateServiceSummary() {
  elements.count.textContent = `${services.length} 项`;
  elements.empty.hidden = services.length > 0;
}

function renderServices() {
  elements.services.replaceChildren();
  services.forEach((service, index) => {
    const row = elements.template.content.firstElementChild.cloneNode(true);
    const name = row.querySelector('.service-name');
    const kind = row.querySelector('.service-kind');
    const target = row.querySelector('.service-target');
    const targetField = row.querySelector('.target-field');
    const remove = row.querySelector('.remove-service');

    name.value = service.name;
    kind.value = service.kind;
    target.value = service.target ?? '';
    const syncTarget = () => {
      const disabled = kind.value === 'socks5';
      target.disabled = disabled;
      target.required = !disabled;
      targetField.classList.toggle('disabled', disabled);
      services[index].kind = kind.value;
      services[index].target = disabled ? null : target.value;
    };
    name.addEventListener('input', () => { services[index].name = name.value; });
    kind.addEventListener('change', syncTarget);
    target.addEventListener('input', () => { services[index].target = target.value; });
    remove.addEventListener('click', () => {
      services.splice(index, 1);
      renderServices();
    });
    syncTarget();
    elements.services.append(row);
  });
  updateServiceSummary();
}

function applyConfig(response) {
  const config = response.config;
  elements.address.value = config.server.address;
  elements.serverName.value = config.server.name ?? '';
  elements.certificate.value = config.server.ca_certificate ?? '';
  elements.key.value = config.key;
  elements.path.textContent = response.path;
  elements.path.title = response.path;
  services = config.services.map((service) => ({ ...service }));
  renderServices();
}

function collectConfig() {
  return {
    key: elements.key.value.trim(),
    server: {
      address: elements.address.value.trim(),
      name: optional(elements.serverName.value),
      ca_certificate: optional(elements.certificate.value)
    },
    services: services.map((service) => ({
      name: service.name.trim(),
      kind: service.kind,
      target: service.kind === 'socks5' ? null : optional(service.target ?? '')
    }))
  };
}

async function save(event) {
  event.preventDefault();
  if (!elements.form.reportValidity()) return;
  const buttons = document.querySelectorAll('[type="submit"]');
  buttons.forEach((button) => { button.disabled = true; });
  elements.notice.hidden = true;
  try {
    const response = await request('/api/config', {
      method: 'PUT',
      body: JSON.stringify(collectConfig())
    });
    applyConfig(response);
    showNotice('配置已保存', 'success');
    await refreshStatus();
  } catch (error) {
    showNotice(error instanceof Error ? error.message : '保存配置失败', 'error');
  } finally {
    buttons.forEach((button) => { button.disabled = false; });
  }
}

async function refreshStatus() {
  try {
    const status = await request('/api/status');
    const labels = {
      starting: '正在启动',
      unconfigured: '等待配置',
      connecting: '正在连接',
      connected: '已连接',
      reconnecting: '等待重连',
      stopped: '已停止'
    };
    elements.statusLabel.textContent = labels[status.state] ?? status.state;
    const detail = status.message
      || [status.server, status.device_id].filter(Boolean).join(' · ')
      || (status.retry_seconds ? `${status.retry_seconds} 秒后重试` : '');
    elements.statusDetail.textContent = detail;
    elements.statusDetail.title = detail;
    elements.statusDot.className = status.state === 'connected'
      ? 'connected'
      : (status.state === 'reconnecting' || status.state === 'stopped' ? 'error' : '');
  } catch {
    elements.statusLabel.textContent = '界面连接已断开';
    elements.statusDetail.textContent = '';
    elements.statusDot.className = 'error';
    window.clearInterval(statusTimer);
  }
}

async function bootstrap() {
  try {
    const session = await fetch('/api/session').then((response) => response.json());
    token = session.token;
    const [config] = await Promise.all([request('/api/config'), refreshStatus()]);
    applyConfig(config);
    statusTimer = window.setInterval(refreshStatus, 1500);
  } catch (error) {
    showNotice(error instanceof Error ? error.message : '加载客户端配置失败', 'error');
  }
}

elements.form.addEventListener('submit', save);
document.querySelector('#add-service').addEventListener('click', () => {
  services.push({ name: '', kind: 'tcp', target: '127.0.0.1:' });
  renderServices();
  elements.services.lastElementChild?.querySelector('.service-name')?.focus();
});
document.querySelector('#toggle-key').addEventListener('click', (event) => {
  const visible = elements.key.type === 'text';
  elements.key.type = visible ? 'password' : 'text';
  event.currentTarget.textContent = visible ? '显示' : '隐藏';
});
document.querySelector('#generate-key').addEventListener('click', async () => {
  try {
    elements.key.value = (await request('/api/key', { method: 'POST' })).key;
  } catch (error) {
    showNotice(error instanceof Error ? error.message : '生成密钥失败', 'error');
  }
});
document.querySelector('#shutdown').addEventListener('click', async () => {
  if (!window.confirm('确定退出 GateRust 客户端？')) return;
  try {
    await request('/api/shutdown', { method: 'POST' });
    window.clearInterval(statusTimer);
    elements.statusLabel.textContent = '客户端已退出';
    elements.statusDetail.textContent = '';
    elements.statusDot.className = 'error';
  } catch (error) {
    showNotice(error instanceof Error ? error.message : '退出客户端失败', 'error');
  }
});

bootstrap();
