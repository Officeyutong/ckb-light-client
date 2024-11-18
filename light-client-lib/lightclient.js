self.onerror = event => {
    console.error(event);
}

let wasmModule;
self.onmessage = async (evt) => {
    const data = evt.data;
    if (wasmModule === undefined) {
        wasmModule = await import(data.entryJsPath);
        await wasmModule.default(data.wasmModulePath);
        wasmModule.set_shared_array(data.inputBuffer, data.outputBuffer);
        wasmModule.open_database("testdb");
        self.postMessage({});
        return;
    }
    const result = wasmModule.take_while_benchmark(evt.data.count);
    self.postMessage({ result });
}
