import { useState } from "react";
import Sidebar from "./Sidebar";
import Toolbar from "./Toolbar";
import TaskTimeline from "./TaskTimeline";
import TaskDetail from "./TaskDetail";
import SettingsPanel from "./SettingsPanel";

export default function MainLayout() {
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [view, setView] = useState<"timeline" | "detail" | "settings">("timeline");

  const handleSelectTask = (id: string) => {
    setSelectedTaskId(id);
    setView("detail");
  };

  const handleBack = () => {
    setView("timeline");
    setSelectedTaskId(null);
  };

  return (
    <div className="flex h-screen bg-white overflow-hidden">
      {/* Left sidebar — Sourcetree style nav */}
      <Sidebar />

      {/* Main content area */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Top toolbar */}
        <Toolbar
          onBack={view !== "timeline" ? handleBack : undefined}
          onSettings={() => setView("settings")}
        />

        {/* Content */}
        <div className="flex-1 overflow-hidden">
          {view === "settings" ? (
            <SettingsPanel onClose={() => setView("timeline")} />
          ) : view === "detail" && selectedTaskId ? (
            <TaskDetail
              taskId={selectedTaskId}
              onTaskUpdated={() => {}}
              onBack={handleBack}
            />
          ) : (
            <TaskTimeline
              selectedId={selectedTaskId}
              onSelect={handleSelectTask}
            />
          )}
        </div>
      </div>
    </div>
  );
}
