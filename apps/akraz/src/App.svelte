<script lang="ts">
  import { onMount } from 'svelte';

  import { daemonState } from './lib/state/daemonState.svelte';
  import { diagnosticsState } from './lib/state/diagnosticsState.svelte';
  import { identityState } from './lib/state/identityState.svelte';
  import { permissionState } from './lib/state/permissionState.svelte';
  import { settingsState } from './lib/state/settingsState.svelte';
  import {
    formatDiagnosticsSnapshot,
    screenTopologySummary,
    unavailableSectionsSummary,
  } from './lib/diagnostics/diagnosticsSnapshot';
  import { selectTrustedPeerSessionDraft } from './lib/session/sessionDraft';
  import type {
    ControlMode,
    DaemonLifecyclePhase,
    IdentityTrustedPeer,
    PlatformCapabilities,
    PeerStatus,
    ScreenEdge,
    SessionConnectParams,
  } from './lib/api/types';

  const modeLabels: Record<ControlMode, string> = {
    Local: '로컬',
    EnteringRemote: '넘어가는 중',
    Remote: '원격 제어 중',
    LeavingRemote: '돌아오는 중',
    Suspended: '일시정지',
  };

  const phaseLabels: Record<DaemonLifecyclePhase, string> = {
    not_running: '꺼져 있음',
    starting: '시작 중',
    running: '연결됨',
    unreachable: '응답 없음',
    failed: '확인 실패',
  };

  const capabilityRows: Array<{ key: keyof PlatformCapabilities; label: string }> = [
    { key: 'canCapturePointer', label: '마우스 잡기' },
    { key: 'canCaptureKeyboard', label: '키보드 잡기' },
    { key: 'canInjectPointer', label: '마우스 보내기' },
    { key: 'canInjectKeyboard', label: '키보드 보내기' },
  ];

  const edgeLabels: Record<ScreenEdge, string> = {
    left: '왼쪽',
    right: '오른쪽',
    top: '위',
    bottom: '아래',
  };

  const edgeOptions: ScreenEdge[] = ['left', 'right', 'top', 'bottom'];
  const identityCapabilities: Array<{ bit: number; label: string }> = [
    { bit: 1, label: '마우스' },
    { bit: 2, label: '키보드' },
    { bit: 4, label: '클립보드' },
    { bit: 8, label: '화면 배치' },
  ];

  let sessionPeerId = $state('');
  let sessionLocalDeviceId = $state('');
  let sessionAddress = $state('');
  let diagnosticsCopyMessage = $state('');

  onMount(() => {
    void daemonState.refresh();
    void identityState.load();
    void settingsState.load();
  });

  function canStartDaemon() {
    const phase = daemonState.snapshot?.phase;
    return phase === undefined || phase === 'not_running';
  }

  function canStopDaemon() {
    return daemonState.snapshot?.phase === 'running' && daemonState.snapshot.managedPid !== null;
  }

  function statusMessage() {
    if (daemonState.lastError) {
      return daemonState.lastError;
    }

    return daemonState.snapshot?.detail ?? '백그라운드 연결을 시작할 수 있어.';
  }

  function settingsMessage() {
    if (settingsState.lastError) {
      return settingsState.lastError;
    }
    if (settingsState.saved) {
      return '저장됨';
    }
    return '저장 전';
  }

  function identityMessage() {
    if (identityState.operation === 'load') {
      return '불러오는 중';
    }
    if (identityState.operation === 'trust') {
      return '등록 중';
    }
    if (identityState.operation === 'forget') {
      return '삭제 중';
    }
    if (identityState.lastError) {
      return identityState.lastError;
    }
    if (identityState.trusted) {
      return `${identityState.trusted.displayName} 등록됨`;
    }
    return identityState.local ? '준비됨' : '대기 중';
  }

  function permissionMessage() {
    if (permissionState.operation === 'probe') {
      return '확인 중';
    }
    if (permissionState.lastError) {
      return permissionState.lastError;
    }
    if (permissionState.probe) {
      return permissionState.probe.issues.length === 0 ? '정상' : `${permissionState.probe.issues.length}개 필요`;
    }
    return '대기 중';
  }

  function diagnosticsMessage() {
    if (diagnosticsState.operation === 'snapshot') {
      return '생성 중';
    }
    if (diagnosticsCopyMessage) {
      return diagnosticsCopyMessage;
    }
    if (diagnosticsState.lastError) {
      return diagnosticsState.lastError;
    }
    return diagnosticsState.snapshot ? '준비됨' : '대기 중';
  }

  function diagnosticsJson() {
    return diagnosticsState.snapshot ? formatDiagnosticsSnapshot(diagnosticsState.snapshot) : '';
  }

  async function generateDiagnostics() {
    diagnosticsCopyMessage = '';
    await diagnosticsState.refresh();
  }

  async function copyDiagnostics() {
    const json = diagnosticsJson();
    if (!json) {
      return;
    }

    try {
      await navigator.clipboard.writeText(json);
      diagnosticsCopyMessage = '복사됨';
    } catch (error) {
      diagnosticsCopyMessage = error instanceof Error ? error.message : '복사 실패';
    }
  }

  function canTrustIdentity() {
    return identityState.local !== null && !identityState.isBusy && identityState.peerDocumentReady;
  }

  function canForgetIdentity() {
    return identityState.local !== null && !identityState.isBusy;
  }

  function isForgettingIdentity(peerId: string) {
    return identityState.operation === 'forget' && identityState.forgettingPeerId === peerId;
  }

  function capabilityLabel(capabilities: number) {
    const labels = identityCapabilities
      .filter((capability) => (capabilities & capability.bit) === capability.bit)
      .map((capability) => capability.label);

    return labels.length > 0 ? labels.join(' · ') : '없음';
  }

  function trustedPeerLabel(peer: IdentityTrustedPeer) {
    return `${peer.displayName} (${peer.peerId})`;
  }

  function trustedPeerSelectValue(peerId: string) {
    const normalizedPeerId = peerId.trim();
    return identityState.trustedPeers.some((peer) => peer.peerId === normalizedPeerId) ? normalizedPeerId : '';
  }

  function selectSessionTrustedPeer(peerId: string) {
    const draft = selectTrustedPeerSessionDraft(
      {
        peerId: sessionPeerId,
        localDeviceId: sessionLocalDeviceId,
        address: sessionAddress,
      },
      peerId,
      settingsState.manualPeerAddress(peerId),
      identityState.local?.deviceId ?? null,
    );

    sessionPeerId = draft.peerId;
    sessionLocalDeviceId = draft.localDeviceId;
    sessionAddress = draft.address;
  }

  function updateSessionAddress(address: string) {
    sessionAddress = address;
    settingsState.updateManualPeerAddress(sessionPeerId, address);
  }

  function selectEdgeTrustedPeer(index: number, peerId: string) {
    if (peerId.length === 0) {
      return;
    }

    settingsState.updateEdgeBinding(index, 'peerId', peerId);
  }

  function connectedPeers() {
    return daemonState.status?.peers.filter((peer) => peer.connected) ?? [];
  }

  function connectedPeerCount() {
    return connectedPeers().length;
  }

  function firstConnectedPeer() {
    return connectedPeers()[0] ?? null;
  }

  function peerDisplayName(peer: PeerStatus) {
    const displayName = peer.displayName.trim();
    return displayName.length > 0 ? displayName : peer.peerId;
  }

  function peerConnectionLabel(peer: PeerStatus) {
    return peer.connected ? '연결됨' : '대기 중';
  }

  function hasConnectedPeer() {
    return connectedPeerCount() > 0;
  }

  function sessionFieldsAreReady() {
    return (
      sessionPeerId.trim().length > 0 &&
      sessionLocalDeviceId.trim().length > 0 &&
      sessionAddress.trim().length > 0
    );
  }

  function canConnectSession() {
    return daemonState.status !== null && !daemonState.isBusy && !hasConnectedPeer() && sessionFieldsAreReady();
  }

  function canDisconnectSession() {
    return daemonState.status !== null && !daemonState.isBusy && hasConnectedPeer();
  }

  function canReleaseAllInputs() {
    return daemonState.status !== null && !daemonState.isBusy;
  }

  function sessionMessage() {
    if (daemonState.operation === 'connectSession') {
      return '연결 중';
    }
    if (daemonState.operation === 'disconnectSession') {
      return '해제 중';
    }
    if (daemonState.operation === 'releaseAllInputs') {
      return '입력 해제 중';
    }
    if (daemonState.lastError) {
      return daemonState.lastError;
    }
    if (!daemonState.status) {
      return '데몬 대기';
    }

    const count = connectedPeerCount();
    if (count > 0) {
      const peer = firstConnectedPeer();
      return count === 1 && peer ? `${peerDisplayName(peer)} 연결됨` : `${count}대 연결됨`;
    }

    return '대기 중';
  }

  async function connectSession() {
    const params: SessionConnectParams = {
      peerId: sessionPeerId.trim(),
      localDeviceId: sessionLocalDeviceId.trim(),
      address: sessionAddress.trim(),
    };

    await daemonState.connectSession(params);
  }
</script>

<main class="shell">
  <section class="status-panel" aria-labelledby="home-title">
    <div class="title-row">
      <div>
        <p class="eyebrow">Akraz</p>
        <h1 id="home-title">입력 공유 상태</h1>
      </div>
      <div class="action-row" aria-label="데몬 제어">
        <button
          type="button"
          class="control-button secondary"
          disabled={daemonState.isBusy}
          onclick={() => daemonState.refresh()}
        >
          {daemonState.operation === 'refresh' ? '확인 중' : '새로고침'}
        </button>
        <button
          type="button"
          class="control-button"
          disabled={daemonState.isBusy || settingsState.isBusy || !canStartDaemon()}
          onclick={() => daemonState.start(settingsState.startOptions)}
        >
          {daemonState.operation === 'start' ? '시작 중' : '시작'}
        </button>
        <button
          type="button"
          class="control-button danger"
          disabled={daemonState.isBusy || !canStopDaemon()}
          onclick={() => daemonState.stop()}
        >
          {daemonState.operation === 'stop' ? '끄는 중' : '중지'}
        </button>
      </div>
    </div>

    {#if daemonState.status}
      <div class="status-summary" aria-live="polite">
        <span class="status-dot ok" aria-hidden="true"></span>
        <div>
          <p class="summary-label">데몬 연결됨</p>
          <p class="summary-value">
            {modeLabels[daemonState.status.mode]} · v{daemonState.status.daemonVersion} · protocol
            {daemonState.status.protocol.major}.{daemonState.status.protocol.minor}
          </p>
        </div>
      </div>

      <div class="grid">
        <section class="section-block" aria-labelledby="capabilities-title">
          <div class="section-heading-row">
            <h2 id="capabilities-title">입출력 권한</h2>
            <span class:error-text={permissionState.lastError}>{permissionMessage()}</span>
          </div>
          <div class="capability-list">
            {#each capabilityRows as row}
              <div class="capability-row">
                <span>{row.label}</span>
                <strong class:ok={daemonState.status.capabilities[row.key]}>
                  {daemonState.status.capabilities[row.key] ? '가능' : '필요'}
                </strong>
              </div>
            {/each}
          </div>

          {#if permissionState.probe}
            <div class="permission-summary">
              <span>어댑터</span>
              <strong>{permissionState.probe.adapterName}</strong>
            </div>
            {#if permissionState.probe.issues.length === 0}
              <p class="muted permission-note">막힌 권한이 없어.</p>
            {:else}
              <ul class="permission-issue-list" aria-label="권한 문제">
                {#each permissionState.probe.issues as issue (issue.code)}
                  <li>
                    <code>{issue.code}</code>
                    <span>{issue.message}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}

          <div class="settings-actions">
            <button
              type="button"
              class="control-button secondary compact"
              disabled={permissionState.isBusy || daemonState.isBusy}
              onclick={() => permissionState.refresh()}
            >
              {permissionState.operation === 'probe' ? '확인 중' : '권한 확인'}
            </button>
          </div>
        </section>

        <section class="section-block" aria-labelledby="peers-title">
          <h2 id="peers-title">연결된 기기</h2>
          {#if daemonState.status.peers.length === 0}
            <p class="muted">아직 연결된 기기가 없어.</p>
          {:else}
            <ul class="peer-list">
              {#each daemonState.status.peers as peer (peer.peerId)}
                <li>
                  <span class="peer-main">
                    <span class="peer-name">{peerDisplayName(peer)}</span>
                    <code class="peer-id">{peer.peerId}</code>
                  </span>
                  <strong>{peerConnectionLabel(peer)}</strong>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </div>
    {:else}
      <div class="status-summary error" aria-live="polite">
        <span class="status-dot error" aria-hidden="true"></span>
        <div>
          <p class="summary-label">
            {daemonState.snapshot ? phaseLabels[daemonState.snapshot.phase] : '확인 전'}
          </p>
          <p class="summary-value">{statusMessage()}</p>
        </div>
      </div>
    {/if}

    <section class="section-block diagnostics-block" aria-labelledby="diagnostics-title">
      <div class="section-heading-row">
        <h2 id="diagnostics-title">진단</h2>
        <span class:error-text={diagnosticsState.lastError}>{diagnosticsMessage()}</span>
      </div>

      {#if diagnosticsState.snapshot}
        <dl class="diagnostics-facts">
          <div>
            <dt>생성</dt>
            <dd>{diagnosticsState.snapshot.generatedBy} · v{diagnosticsState.snapshot.toolVersion}</dd>
          </div>
          <div>
            <dt>데몬</dt>
            <dd>
              {modeLabels[diagnosticsState.snapshot.daemon.mode]} ·
              {diagnosticsState.snapshot.daemon.connectedPeerCount}/{diagnosticsState.snapshot.daemon.peerCount}
            </dd>
          </div>
          <div>
            <dt>화면</dt>
            <dd>{screenTopologySummary(diagnosticsState.snapshot)}</dd>
          </div>
          <div>
            <dt>미포함</dt>
            <dd>{unavailableSectionsSummary(diagnosticsState.snapshot)}</dd>
          </div>
        </dl>

        <label class="document-field diagnostics-json">
          <span>진단 JSON</span>
          <textarea
            readonly
            rows="12"
            value={diagnosticsJson()}
            aria-label="진단 JSON"
            spellcheck="false"
          ></textarea>
        </label>
      {:else}
        <p class="muted">진단 자료를 만들 수 있어.</p>
      {/if}

      <div class="settings-actions">
        <button
          type="button"
          class="control-button secondary"
          disabled={diagnosticsState.isBusy}
          onclick={generateDiagnostics}
        >
          {diagnosticsState.operation === 'snapshot' ? '생성 중' : '생성'}
        </button>
        <button
          type="button"
          class="control-button"
          disabled={diagnosticsState.isBusy || diagnosticsState.snapshot === null}
          onclick={copyDiagnostics}
        >
          복사
        </button>
      </div>
    </section>

    <section class="section-block identity-block" aria-labelledby="identity-title">
      <div class="section-heading-row">
        <h2 id="identity-title">기기 등록</h2>
        <span class:error-text={identityState.lastError}>{identityMessage()}</span>
      </div>

      <div class="identity-grid">
        <div class="identity-column">
          <div class="identity-heading">
            <h3>내 기기</h3>
            <button
              type="button"
              class="control-button secondary compact"
              disabled={identityState.isBusy}
              onclick={() => identityState.load()}
            >
              {identityState.operation === 'load' ? '확인 중' : '새로고침'}
            </button>
          </div>

          {#if identityState.local}
            <dl class="identity-facts">
              <div>
                <dt>이름</dt>
                <dd>{identityState.local.displayName}</dd>
              </div>
              <div>
                <dt>기기 ID</dt>
                <dd><code>{identityState.local.deviceId}</code></dd>
              </div>
              <div>
                <dt>Fingerprint</dt>
                <dd><code>{identityState.local.fingerprint}</code></dd>
              </div>
              <div>
                <dt>입력</dt>
                <dd>{capabilityLabel(identityState.local.capabilities)}</dd>
              </div>
            </dl>
            <label class="document-field">
              <span>내 기기 코드</span>
              <textarea
                readonly
                rows="8"
                value={identityState.local.documentJson}
                aria-label="내 기기 코드"
                spellcheck="false"
              ></textarea>
            </label>
          {:else}
            <p class="muted">아직 내 기기 정보가 없어.</p>
          {/if}
        </div>

        <div class="identity-column">
          <h3>상대 기기</h3>
          <label class="document-field">
            <span>상대 기기 코드</span>
            <textarea
              rows="8"
              value={identityState.peerDocumentJson}
              placeholder="akraz.peerIdentity JSON"
              aria-label="상대 기기 코드"
              spellcheck="false"
              disabled={identityState.isBusy}
              oninput={(event) => identityState.updatePeerDocumentJson(event.currentTarget.value)}
            ></textarea>
          </label>

          {#if identityState.trusted}
            <div class="trust-result" aria-live="polite">
              <strong>{identityState.trusted.displayName}</strong>
              <code>{identityState.trusted.fingerprint}</code>
              <span>{capabilityLabel(identityState.trusted.capabilities)}</span>
            </div>
          {/if}

          <div class="settings-actions">
            <button type="button" class="control-button" disabled={!canTrustIdentity()} onclick={() => identityState.trust()}>
              {identityState.operation === 'trust' ? '등록 중' : '등록'}
            </button>
          </div>
        </div>
      </div>

      <div class="trusted-peer-panel">
        <h3>등록된 기기</h3>
        {#if identityState.trustedPeers.length > 0}
          <ul class="trusted-peer-list">
            {#each identityState.trustedPeers as peer (peer.peerId)}
              <li>
                <span class="trusted-peer-main">
                  <strong>{peer.displayName}</strong>
                  <code>{peer.peerId}</code>
                  <code>{peer.fingerprint}</code>
                  <span>{capabilityLabel(peer.capabilities)}</span>
                </span>
                <button
                  type="button"
                  class="control-button danger compact"
                  disabled={!canForgetIdentity()}
                  onclick={() => identityState.forget(peer.peerId)}
                >
                  {isForgettingIdentity(peer.peerId) ? '삭제 중' : '삭제'}
                </button>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="muted">등록된 기기가 없어.</p>
        {/if}
      </div>
    </section>

    <section class="section-block session-block" aria-labelledby="session-title">
      <div class="section-heading-row">
        <h2 id="session-title">기기 연결</h2>
        <span class:error-text={daemonState.lastError}>{sessionMessage()}</span>
      </div>

      <div class="session-row">
        <label>
          <span>기기 ID</span>
          <select
            value={trustedPeerSelectValue(sessionPeerId)}
            disabled={daemonState.isBusy || hasConnectedPeer() || identityState.trustedPeers.length === 0}
            onchange={(event) => selectSessionTrustedPeer(event.currentTarget.value)}
            aria-label="등록된 기기 선택"
          >
            <option value="">직접 입력</option>
            {#each identityState.trustedPeers as peer (peer.peerId)}
              <option value={peer.peerId}>{trustedPeerLabel(peer)}</option>
            {/each}
          </select>
          <input
            type="text"
            bind:value={sessionPeerId}
            placeholder="linux-laptop"
            spellcheck="false"
            disabled={daemonState.isBusy || hasConnectedPeer()}
          />
        </label>

        <label>
          <span>내 기기 ID</span>
          <input
            type="text"
            bind:value={sessionLocalDeviceId}
            placeholder="windows-desktop"
            spellcheck="false"
            disabled={daemonState.isBusy || hasConnectedPeer()}
          />
        </label>

        <label>
          <span>주소</span>
          <input
            type="text"
            value={sessionAddress}
            placeholder="127.0.0.1:4455"
            spellcheck="false"
            disabled={daemonState.isBusy || hasConnectedPeer()}
            oninput={(event) => updateSessionAddress(event.currentTarget.value)}
          />
        </label>
      </div>

      <div class="settings-actions">
        <button type="button" class="control-button" disabled={!canConnectSession()} onclick={connectSession}>
          {daemonState.operation === 'connectSession' ? '연결 중' : '연결'}
        </button>
        <button
          type="button"
          class="control-button secondary"
          disabled={!canDisconnectSession()}
          onclick={() => daemonState.disconnectSession()}
        >
          {daemonState.operation === 'disconnectSession' ? '해제 중' : '해제'}
        </button>
        <button
          type="button"
          class="control-button danger"
          disabled={!canReleaseAllInputs()}
          onclick={() => daemonState.releaseAllInputs()}
        >
          {daemonState.operation === 'releaseAllInputs' ? '해제 중' : '입력 해제'}
        </button>
      </div>
    </section>

    <section class="section-block settings-block" aria-labelledby="settings-title">
      <div class="section-heading-row">
        <h2 id="settings-title">수동 연결</h2>
        <span class:error-text={settingsState.lastError}>{settingsMessage()}</span>
      </div>

      <label class="toggle-row">
        <input
          type="checkbox"
          checked={settingsState.settings.captureInput}
          onchange={(event) => settingsState.updateCaptureInput(event.currentTarget.checked)}
        />
        <span>입력 잡기</span>
      </label>

      <label class="peer-listen-row">
        <span>받는 주소</span>
        <input
          type="text"
          value={settingsState.settings.peerListenAddress}
          placeholder="0.0.0.0:4455"
          spellcheck="false"
          disabled={settingsState.isBusy}
          oninput={(event) => settingsState.updatePeerListenAddress(event.currentTarget.value)}
        />
      </label>

      <div class="edge-list" aria-label="화면 끝 연결">
        {#each settingsState.settings.edgeBindings as binding, index}
          <div class="edge-row">
            <label>
              <span>내 화면</span>
              <select
                value={binding.localEdge}
                onchange={(event) => settingsState.updateEdgeBinding(index, 'localEdge', event.currentTarget.value)}
              >
                {#each edgeOptions as edge}
                  <option value={edge}>{edgeLabels[edge]}</option>
                {/each}
              </select>
            </label>

            <label class="peer-field">
              <span>기기 ID</span>
              <select
                value={trustedPeerSelectValue(binding.peerId)}
                disabled={identityState.trustedPeers.length === 0}
                onchange={(event) => selectEdgeTrustedPeer(index, event.currentTarget.value)}
                aria-label="등록된 기기 선택"
              >
                <option value="">직접 입력</option>
                {#each identityState.trustedPeers as peer (peer.peerId)}
                  <option value={peer.peerId}>{trustedPeerLabel(peer)}</option>
                {/each}
              </select>
              <input
                type="text"
                value={binding.peerId}
                placeholder="linux-laptop"
                spellcheck="false"
                onchange={(event) => settingsState.updateEdgeBinding(index, 'peerId', event.currentTarget.value)}
              />
            </label>

            <label>
              <span>상대 화면</span>
              <select
                value={binding.remoteEdge}
                onchange={(event) => settingsState.updateEdgeBinding(index, 'remoteEdge', event.currentTarget.value)}
              >
                {#each edgeOptions as edge}
                  <option value={edge}>{edgeLabels[edge]}</option>
                {/each}
              </select>
            </label>

            <button
              type="button"
              class="icon-button"
              aria-label="연결 삭제"
              onclick={() => settingsState.removeEdgeBinding(index)}
            >
              ×
            </button>
          </div>
        {/each}
      </div>

      {#if identityState.trustedPeers.length > 0}
        <div class="manual-address-list" aria-label="기기 주소">
          <h3>기기 주소</h3>
          {#each identityState.trustedPeers as peer (peer.peerId)}
            <label class="manual-address-row">
              <span>{trustedPeerLabel(peer)}</span>
              <input
                type="text"
                value={settingsState.manualPeerAddress(peer.peerId)}
                placeholder="127.0.0.1:4455"
                spellcheck="false"
                disabled={settingsState.isBusy}
                oninput={(event) => settingsState.updateManualPeerAddress(peer.peerId, event.currentTarget.value)}
              />
            </label>
          {/each}
        </div>
      {/if}

      <div class="settings-actions">
        <button
          type="button"
          class="control-button secondary"
          disabled={settingsState.isBusy}
          onclick={() => settingsState.addEdgeBinding()}
        >
          추가
        </button>
        <button
          type="button"
          class="control-button"
          disabled={settingsState.isBusy}
          onclick={() => settingsState.save()}
        >
          {settingsState.operation === 'save' ? '저장 중' : '저장'}
        </button>
      </div>
    </section>
  </section>
</main>
