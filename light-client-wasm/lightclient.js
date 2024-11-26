onerror = err => {
    console.error(err);
}
let wasmModule;
onmessage = async (evt) => {
    const data = evt.data;
    if (wasmModule === undefined) {
        wasmModule = await import(data.entryJsPath);
        await wasmModule.default({ module_or_path: data.wasmModulePath });
        wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
        await wasmModule.light_client(data.netName, "debug");
        self.postMessage({});
        return;
    }
    self.postMessage(wasmModule[evt.data.name](...evt.data.args))
};
