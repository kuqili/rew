import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { checkFirstRun } from "./lib/tauri";
import SetupWizard from "./components/SetupWizard";
import MainLayout from "./components/MainLayout";

export default function App() {
  const [isFirstRun, setIsFirstRun] = useState<boolean | null>(null);

  useEffect(() => {
    checkFirstRun().then(setIsFirstRun).catch(() => setIsFirstRun(false));

    // Listen for setup completion
    const unlisten = listen("setup-complete", () => {
      setIsFirstRun(false);
    });

    return () => { unlisten.then(fn => fn()); };
  }, []);

  // Loading
  if (isFirstRun === null) {
    return (
      <div className="flex items-center justify-center h-screen bg-bg-primary">
        <div className="text-text-muted text-sm">Loading...</div>
      </div>
    );
  }

  if (isFirstRun) {
    return <SetupWizard onComplete={() => setIsFirstRun(false)} />;
  }

  return <MainLayout />;
}
