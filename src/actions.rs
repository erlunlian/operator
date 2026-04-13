use gpui::actions;

actions!(
    operator,
    [
        NewWorkspace,
        NewTab,
        CloseTab,
        SplitPane,
        SplitPaneVertical,
        ToggleSidebar,
        ToggleDiffPanel,
        ToggleFilesPanel,
        TogglePrPanel,
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
