import { DbWorkerInitializeOptions } from "./types";

onerror = event => {
    console.error(event);
}

onmessage = async (evt) => {
    const wasmModule = (await import("light-client-db-worker")).default;

    const data = evt.data as DbWorkerInitializeOptions;
    wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
    self.postMessage({});
    await wasmModule.main_loop(data.logLevel);
}

export {};
