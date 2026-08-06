/**
 * The app shell. Deliberately bare: the screens arrive with the tasks that
 * need them, and this only has to prove the toolchain renders and typechecks.
 */
function App() {
  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-2">
      <h1 className="text-2xl font-semibold">Fluid</h1>
      <p className="text-sm opacity-70">
        Crystalline stores what was learned; Fluid is where you think with it.
      </p>
      <p className="text-xs opacity-50">v{import.meta.env.VITE_APP_VERSION}</p>
    </main>
  );
}

export default App;
