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
        await wasmModule.light_client(data.networkFlag, data.logLevel);
        self.postMessage({});
        loaded = true;
        return;
    }
    const data = evt.data as LightClientFunctionCall;
    try {
        self.postMessage({
            ok: true,
            data: ((wasmModule as any)[data.name])(...evt.data.args)
        })
    } catch (e) {
        self.postMessage({
            ok: false,
            error: `${e}`
        })
        console.error(e);
    }
};

export { };
