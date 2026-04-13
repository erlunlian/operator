use gpui::actions;

actions!(
    operator,
    [
        NewWorkspace,
        NewTab,
        NewEditorTab,
        CloseTab,
        SplitPane,
        SplitPaneVertical,
        ToggleSidebar,
        ToggleDiffPanel,
        NextTab,
        PrevTab,
        SaveFile,
        ToggleSettings,
        ToggleCommandCenter,
        FindInFile,
        SearchWorkspace,
        Quit,
    ]
);
