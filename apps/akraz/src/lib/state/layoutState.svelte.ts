import { daemonClient } from "../api/daemonClient";
import { layoutClient } from "../api/layoutClient";
import type {
  DiagnosticsScreenTopology,
  LayoutSettings,
  ScreenEdge,
  ScreenEdgeBinding,
} from "../api/types";

type LayoutOperation = "load" | "save";
type TopologyOperation = "probe";

function defaultLayout(): LayoutSettings {
  return {
    edgeBindings: [],
  };
}

function defaultEdgeBinding(): ScreenEdgeBinding {
  return {
    localEdge: "right",
    peerId: "",
    remoteEdge: "left",
  };
}

export class LayoutState {
  layout = $state<LayoutSettings>(defaultLayout());
  topology = $state<DiagnosticsScreenTopology | null>(null);
  operation = $state<LayoutOperation | null>(null);
  topologyOperation = $state<TopologyOperation | null>(null);
  lastError = $state<string | null>(null);
  topologyError = $state<string | null>(null);
  saved = $state(false);

  get isBusy(): boolean {
    return this.operation !== null;
  }

  get isTopologyBusy(): boolean {
    return this.topologyOperation !== null;
  }

  addEdgeBinding() {
    this.layout.edgeBindings = [...this.layout.edgeBindings, defaultEdgeBinding()];
    this.saved = false;
  }

  removeEdgeBinding(index: number) {
    this.layout.edgeBindings = this.layout.edgeBindings.filter(
      (_, itemIndex) => itemIndex !== index,
    );
    this.saved = false;
  }

  updateEdgeBinding(index: number, field: keyof ScreenEdgeBinding, value: string) {
    this.layout.edgeBindings = this.layout.edgeBindings.map((binding, itemIndex) => {
      if (itemIndex !== index) {
        return binding;
      }

      return {
        ...binding,
        [field]: field === "peerId" ? value : (value as ScreenEdge),
      };
    });
    this.saved = false;
  }

  moveEdgeBinding(index: number, localEdge: ScreenEdge, remoteEdge: ScreenEdge) {
    this.layout.edgeBindings = this.layout.edgeBindings.map((binding, itemIndex) => {
      if (itemIndex !== index) {
        return binding;
      }

      return {
        ...binding,
        localEdge,
        remoteEdge,
      };
    });
    this.saved = false;
  }

  async load(): Promise<LayoutSettings | null> {
    let loadedLayout: LayoutSettings | null = null;
    await this.run("load", async () => {
      loadedLayout = await layoutClient.get();
      this.layout = loadedLayout;
      this.saved = false;
    });

    return loadedLayout;
  }

  async save(): Promise<LayoutSettings | null> {
    let savedLayout: LayoutSettings | null = null;
    await this.run("save", async () => {
      savedLayout = await layoutClient.set(this.layout);
      this.layout = savedLayout;
      this.saved = true;
    });

    return savedLayout;
  }

  async refreshTopology(): Promise<DiagnosticsScreenTopology | null> {
    let topology: DiagnosticsScreenTopology | null = null;
    this.topologyOperation = "probe";
    this.topologyError = null;
    try {
      topology = await daemonClient.screenTopology();
      this.topology = topology;
    } catch (error) {
      this.topologyError = error instanceof Error ? error.message : String(error);
    } finally {
      this.topologyOperation = null;
    }

    return topology;
  }

  private async run(operation: LayoutOperation, action: () => Promise<void>) {
    this.operation = operation;
    this.lastError = null;
    try {
      await action();
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.operation = null;
    }
  }
}

export const layoutState = new LayoutState();
