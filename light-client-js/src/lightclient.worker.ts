import { LightClientFunctionCall, LightClientWorkerInitializeOptions } from "./types";

onerror = err => {
    console.error(err);
}
let loaded = false;
onmessage = async (evt) => {
    const wasmModule = (await import("light-client-wasm")).default;
    if (!loaded) {
        const data = evt.data as LightClientWorkerInitializeOptions;

        wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
        await wasmModule.light_client(data.netName, data.logLevel);
        self.postMessage({});
        loaded = true;
        return;
    }
    const data = evt.data as LightClientFunctionCall;
    self.postMessage(((wasmModule as any)[data.name])(...evt.data.args))
};

export { };
